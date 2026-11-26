use super::*;
use crate::{mod_organization, storage::AppPaths};
use serde_json::json;
use std::fs;

const NEXUS: &str = "hycm5cs-227-1-0-1759501562.7z";

fn setup() -> (tempfile::TempDir, FileEngine, games::Game, games::Game) {
    let temp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(temp.path().join("manager")).unwrap();
    let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
    let register = |name: &str| {
        let root = temp.path().join(name);
        fs::create_dir(&root).unwrap();
        games::register_game(&engine.db, &root, None).unwrap()
    };
    let game = register("Fixture Game");
    let other = register("Other Game");
    (temp, engine, game, other)
}

fn import(
    engine: &FileEngine,
    game: &games::Game,
    filename: &str,
    bound: bool,
) -> mod_library::LibraryEntry {
    let id = uuid::Uuid::new_v4().to_string();
    let bucket = engine.paths.staging().join("imports").join(&id);
    fs::create_dir_all(&bucket).unwrap();
    let source = bucket.join(filename);
    let payload = b"fixture";
    fs::write(&source, payload).unwrap();
    mod_library::ingest_import(
        engine,
        &mod_library::ApprovedArtifact {
            artifact_id: &id,
            path: &source,
            filename,
            size: payload.len() as u64,
            sha256: &crate::util::sha256_file(&source).unwrap(),
        },
        &game.name,
        bound.then_some(game.id.as_str()),
    )
    .unwrap()
}

fn install_row_for_test(
    engine: &FileEngine,
    game: &games::Game,
    id: &str,
    version: Option<&str>,
    metadata: Value,
) {
    engine.db.conn().execute(
        "INSERT INTO installs(id,game_id,name,version,install_root,metadata_json,installed_at,updated_at)
         VALUES (?1,?2,'Original',?3,?4,?5,'now','now')",
        params![id,game.id,version,game.root_path,metadata.to_string()],
    ).unwrap();
}

fn archive_json(engine: &FileEngine, game: &games::Game) -> Value {
    let entries = mod_library::list_for_game(&engine.db, &game.id).unwrap();
    serde_json::to_value(archive_display(&engine.db, &game.id, entries).unwrap()).unwrap()
}

fn patch(value: Value) -> Patch {
    serde_json::from_value(value).unwrap()
}

#[test]
fn version_filename_requires_nexus_shape_or_an_explicit_dotted_version() {
    for (filename, version) in [
        (NEXUS, "1.0"),
        ("MyMod-v1.2.3.zip", "1.2.3"),
        ("Any_Mod_V2.0.TAR.GZ", "2.0"),
    ] {
        assert_eq!(filename_version(filename).as_deref(), Some(version));
    }
    for filename in [
        "Porsche-2025-001.zip",
        "GT3-911-7.zip",
        "MyMod-1.2.3.zip",
        "Porsche-v2025.zip",
        "MyMod-227-1-0-1234.7z",
    ] {
        assert_eq!(filename_version(filename), None, "{filename}");
    }
}

#[test]
fn visible_unbound_archive_edit_is_atomic_game_scoped_and_keeps_source_untouched() {
    let (_temp, engine, game, other) = setup();
    let entry = import(&engine, &game, NEXUS, false);
    let target: Target = serde_json::from_value(json!({"kind":"archive","id":entry.id})).unwrap();
    assert!(serde_json::from_value::<Patch>(json!({"version":"bogus"})).is_err());
    assert!(serde_json::from_value::<Target>(
        json!({"kind":"archive","id":entry.id,"path":"other"})
    )
    .is_err());
    let local =
        mod_organization::save_mod_tag(&engine.db, &game.id, None, "Local", "#ABCDEF").unwrap();
    let foreign =
        mod_organization::save_mod_tag(&engine.db, &other.id, None, "Foreign", "#ABCDEF").unwrap();
    let long_name = "模".repeat(200);
    update(
        &engine,
        &game.id,
        &target,
        patch(json!({"name":long_name,"tagIds":[local.id,local.id]})),
    )
    .unwrap();
    assert!(update(
        &engine,
        &game.id,
        &target,
        patch(json!({"name":"Must not stick","tagIds":[foreign.id]}))
    )
    .is_err());
    assert!(update(
        &engine,
        &game.id,
        &target,
        patch(json!({"name":"模".repeat(201),"tagIds":[]}))
    )
    .is_err());
    assert!(update(
        &engine,
        &other.id,
        &target,
        patch(json!({"name":"wrong game"}))
    )
    .is_err());
    let result = archive_json(&engine, &game);
    assert_eq!(result[0]["name"], long_name);
    assert_eq!(result[0]["tagIds"], json!([local.id]));
    assert_eq!(result[0]["gameId"], Value::Null);
    assert_eq!(result[0]["version"], "1.0");
    assert!(result[0]["website"].is_null());
    assert_eq!(
        mod_library::list_for_game(&engine.db, &game.id).unwrap()[0].name,
        NEXUS
    );
    mod_organization::delete_mod_tag(&engine.db, &game.id, &local.id).unwrap();
    assert_eq!(archive_json(&engine, &game)[0]["tagIds"], json!([]));
}

#[test]
fn install_edit_is_atomic_and_remains_separate_from_original_name_and_version() {
    let (_temp, engine, game, other) = setup();
    install_row_for_test(
        &engine,
        &game,
        "installation",
        None,
        json!({"artifactPath":format!(r"Z:\old\{NEXUS}")}),
    );
    let target = Target::Install("installation".into());
    let tag =
        mod_organization::save_mod_tag(&engine.db, &game.id, None, "Local", "#ABCDEF").unwrap();
    let foreign =
        mod_organization::save_mod_tag(&engine.db, &other.id, None, "Foreign", "#ABCDEF").unwrap();
    update(
        &engine,
        &game.id,
        &target,
        patch(json!({"name":" Saved ","tagIds":[tag.id]})),
    )
    .unwrap();
    assert!(update(
        &engine,
        &game.id,
        &target,
        patch(json!({"name":"Changed","tagIds":[foreign.id]}))
    )
    .is_err());
    let rows = mod_management::visible_installs(&engine, &game.id).unwrap();
    let original: String = engine
        .db
        .conn()
        .query_row(
            "SELECT name FROM installs WHERE id='installation'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(original, "Original");
    assert_eq!(rows[0].name, "Saved");
    assert_eq!(rows[0].tag_ids, [tag.id]);
    assert_eq!(rows[0].version.as_deref(), Some("1.0"));
    update(&engine, &game.id, &target, patch(json!({"tagIds":[]}))).unwrap();
    assert!(install::get_install(&engine, "installation")
        .unwrap()
        .tag_ids
        .is_empty());
}

#[test]
fn historical_three_dm_metadata_uses_project_id_and_never_substitutes_resource_id_for_version() {
    let (_temp, engine, game, _) = setup();
    let origin = |source: &str, project: &str| {
        json!({"downloadOrigin": {
            "sourceId":source, "projectId":project, "versionId":"378349",
            "artifactId":"ebff121c-e836-4c00-bcc6-99cfd0ec4539"
        }})
    };
    for (id, source, project, version, website) in [
        (
            "installed",
            "3dm",
            "261498",
            Some("1"),
            Some("https://mod.3dmgame.com/mod/261498"),
        ),
        (
            "missing-version",
            "3dm",
            "261362",
            None,
            Some("https://mod.3dmgame.com/mod/261362"),
        ),
        ("different-provider", "local", "261498", None, None),
        ("invalid-project", "3dm", "../261498", None, None),
    ] {
        install_row_for_test(&engine, &game, id, version, origin(source, project));
        let item = install::get_install(&engine, id).unwrap();
        assert_eq!(item.version.as_deref(), version);
        assert_eq!(item.website.as_deref(), website);
    }
}

#[test]
fn historical_version_ids_are_not_interchangeable_between_sources() {
    assert_eq!(
        provider_version("thunderstore", "1.2.3").as_deref(),
        Some("1.2.3")
    );
    for (source, file) in [
        ("nexus", "123456"),
        ("modrinth", "12345678"),
        ("curseforge", "1.2.3"),
        ("thunderstore", "123456"),
    ] {
        assert!(provider_version(source, file).is_none());
    }
}

#[test]
fn legacy_three_dm_archive_supplies_website_even_without_an_install_origin() {
    let (_temp, engine, game, _) = setup();
    let archive = import(&engine, &game, "Fixture.zip", true);
    engine.db.conn().execute("UPDATE mod_library_entries SET source_id='3dm',project_id='261362',version_id='378103',version='1.0.0',project_page=NULL WHERE id=?1", [&archive.id]).unwrap();
    install_row_for_test(
        &engine,
        &game,
        "installed",
        None,
        json!({"libraryEntryId":archive.id,"artifactSha256":archive.sha256}),
    );
    let page = "https://mod.3dmgame.com/mod/261362";
    assert_eq!(archive_json(&engine, &game)[0]["website"], page);
    let item = install::get_install(&engine, "installed").unwrap();
    assert_eq!(item.version.as_deref(), Some("1.0.0"));
    assert_eq!(item.website.as_deref(), Some(page));
}

#[test]
fn historical_version_and_website_use_exact_archive_receipt_before_filename() {
    let (_temp, engine, game, _) = setup();
    let archive = import(&engine, &game, NEXUS, true);
    engine.db.conn().execute("UPDATE mod_library_entries SET version='release-2',project_page='https://example.org/mod?token=secret' WHERE id=?1", [&archive.id]).unwrap();
    let evidence =
        json!({"libraryEntryId":archive.id,"artifactSha256":archive.sha256,"artifactPath":NEXUS});
    install_row_for_test(&engine, &game, "valid", None, evidence.clone());
    install_row_for_test(&engine, &game, "stored", Some("release-3"), evidence);
    install_row_for_test(
        &engine,
        &game,
        "mismatch",
        None,
        json!({"libraryEntryId":archive.id,"artifactSha256":"different","artifactPath":NEXUS}),
    );
    assert_eq!(
        install::get_install(&engine, "valid")
            .unwrap()
            .version
            .as_deref(),
        Some("release-2")
    );
    assert_eq!(
        install::get_install(&engine, "valid")
            .unwrap()
            .website
            .as_deref(),
        Some("https://example.org/mod")
    );
    assert_eq!(
        install::get_install(&engine, "stored")
            .unwrap()
            .version
            .as_deref(),
        Some("release-3")
    );
    let mismatch = install::get_install(&engine, "mismatch").unwrap();
    assert_eq!(mismatch.version.as_deref(), Some("1.0"));
    assert!(mismatch.website.is_none());
}

#[test]
fn resolves_archives_unbound_or_linked_and_falls_back_to_verified_install_root() {
    let (temp, engine, game, other) = setup();
    let archive = import(&engine, &game, NEXUS, false);
    let target = Target::Archive(archive.id.clone());
    let path = resolve_storage_directory(&engine, &game.id, &target).unwrap();
    assert_eq!(
        path.join("archive").join(NEXUS),
        engine.paths.root.join(&archive.archive_relative_path)
    );
    assert!(resolve_storage_directory(&engine, &other.id, &target).is_err());
    install_row_for_test(
        &engine,
        &game,
        "linked",
        None,
        json!({"libraryEntryId":archive.id}),
    );
    let installed = Target::Install("linked".into());
    assert_eq!(
        resolve_storage_directory(&engine, &game.id, &installed).unwrap(),
        path
    );
    fs::remove_file(path.join("archive").join(NEXUS)).unwrap();
    assert_eq!(
        resolve_storage_directory(&engine, &game.id, &installed).unwrap(),
        PathBuf::from(&game.root_path)
    );
    assert!(resolve_storage_directory(&engine, &game.id, &target).is_err());
    engine
        .db
        .conn()
        .execute(
            "UPDATE installs SET install_root=?1 WHERE id='linked'",
            [temp.path().to_string_lossy()],
        )
        .unwrap();
    assert!(resolve_storage_directory(&engine, &game.id, &installed).is_err());
}
