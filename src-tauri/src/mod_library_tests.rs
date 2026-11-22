use super::*;
use crate::{download::DownloadArtifact, sources::ResolvedMod, storage::AppPaths};

fn setup() -> (tempfile::TempDir, FileEngine) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path().join("manager")).unwrap();
    let db = Db::open(&paths.db_file()).unwrap();
    (tmp, FileEngine::new(db, paths))
}

fn register(engine: &FileEngine, id: &str, name: &str) {
    let root = engine.paths.root.parent().unwrap().join(id);
    fs::create_dir(&root).unwrap();
    engine.db.conn().execute(
        "INSERT INTO games(id,name,slug,root_path,created_at,updated_at) VALUES (?1,?2,?1,?3,'now','now')",
        params![id, name, root.to_str().unwrap()],
    ).unwrap();
}

fn catalog_fixture(
    engine: &FileEngine,
    filename: &str,
    bytes: &[u8],
) -> (DownloadArtifact, ResolvedMod) {
    let id = uuid::Uuid::new_v4().to_string();
    let bucket = engine.paths.downloads().join("b".repeat(64));
    fs::create_dir_all(&bucket).unwrap();
    let path = bucket.join(format!("{id}-{filename}"));
    fs::write(&path, bytes).unwrap();
    let artifact = DownloadArtifact {
        id,
        path,
        filename: filename.into(),
        size: bytes.len() as u64,
        sha256: hex::encode(Sha256::digest(bytes)),
        sha512: hex::encode(Sha512::digest(bytes)),
        integrity_verified: true,
        requires_review: true,
        auto_install_allowed: false,
        warnings: vec![],
    };
    let resolved = ResolvedMod {
        source_id: "modrinth".into(),
        project_id: "demo-project".into(),
        version_id: "demo-version".into(),
        name: "My Mod".into(),
        version: "1.0.0".into(),
        download_url: "https://example.invalid/artifact?signature=do-not-keep".into(),
        filename: artifact.filename.clone(),
        size: Some(artifact.size),
        hashes: Default::default(),
        homepage_url: Some("https://example.invalid/project?token=not-persisted".into()),
        warnings: Vec::new(),
    };
    (artifact, resolved)
}

fn catalog(engine: &FileEngine, filename: &str, bytes: &[u8]) -> ArtifactRecord {
    let (artifact, resolved) = catalog_fixture(engine, filename, bytes);
    artifacts::record(engine, &artifact, &resolved).unwrap();
    artifacts::for_id(&engine.db, &artifact.id)
        .unwrap()
        .unwrap()
}

fn category(entry: &LibraryEntry) -> &str {
    entry.archive_relative_path.split('/').nth(1).unwrap()
}

#[test]
fn categories_are_readable_stable_and_disambiguated() {
    let (_tmp, engine) = setup();
    let first = ingest_download(
        &engine,
        &catalog(&engine, "Original.zip", b"one"),
        "Cool Game",
        None,
    )
    .unwrap();
    let repeat = ingest_download(
        &engine,
        &catalog(&engine, "Second.zip", b"two"),
        "cool  GAME",
        None,
    )
    .unwrap();
    assert_eq!(category(&first), "Cool Game");
    assert_eq!(category(&repeat), "Cool Game");
    assert!(first
        .archive_relative_path
        .ends_with("/archive/Original.zip"));
    assert!(first.payload_relative_path.ends_with("/payload"));
    assert!(first
        .archive_relative_path
        .contains(&format!("My Mod--{}/", first.id)));
    let dto = serde_json::to_value(&first).unwrap();
    assert_eq!(dto["artifactId"], first.id);
    assert_eq!(dto["projectPage"], "https://example.invalid/project");
    assert!(dto.get("id").is_none());

    fs::create_dir(engine.paths.mod_library().join("Reserved Game")).unwrap();
    let collided = ingest_download(
        &engine,
        &catalog(&engine, "Third.zip", b"three"),
        "Reserved Game",
        None,
    )
    .unwrap();
    assert!(category(&collided).starts_with("Reserved Game--"));
    assert_eq!(category(&collided).len(), "Reserved Game--".len() + 8);
    register(&engine, "g1", "Cool Game");
    let registered = ingest_download(
        &engine,
        &catalog(&engine, "Bound.zip", b"bound"),
        "ignored",
        Some("g1"),
    )
    .unwrap();
    assert!(category(&registered).starts_with("Cool Game--"));
    assert_eq!(list_for_game(&engine.db, "g1").unwrap().len(), 3);

    engine
        .db
        .conn()
        .execute("UPDATE games SET name='Renamed Game' WHERE id='g1'", [])
        .unwrap();
    let renamed = ingest_download(
        &engine,
        &catalog(&engine, "New.zip", b"new"),
        "ignored",
        Some("g1"),
    )
    .unwrap();
    assert_eq!(category(&registered), category(&renamed));
    assert_eq!(list_for_game(&engine.db, "g1").unwrap().len(), 2);
}

#[test]
fn long_game_names_keep_distinct_categories_with_safe_directory_names() {
    let (_tmp, engine) = setup();
    let prefix = "游戏名".repeat(100);
    let mut directories = Vec::new();
    for edition in ["A", "B"] {
        let name = format!("{prefix} {edition}");
        register(&engine, edition, &name);
        let entry = ingest_download(
            &engine,
            &catalog(&engine, "Fixture.zip", edition.as_bytes()),
            &name,
            Some(edition),
        )
        .unwrap();
        assert_eq!(entry.game_name, name);
        assert_eq!(
            list_for_game(&engine.db, edition).unwrap().as_slice(),
            std::slice::from_ref(&entry)
        );
        let dir = category(&entry);
        assert!(dir.len() <= 80 + "--".len() + 64);
        assert!(engine.paths.mod_library().join(dir).is_dir());
        directories.push(dir.to_owned());
    }
    assert_ne!(directories[0], directories[1]);
}

#[test]
fn explicit_binding_handles_duplicate_registrations_without_transport_or_payload() {
    let (_tmp, engine) = setup();
    let record = catalog(&engine, "Archive.zip", b"original bytes");
    let entry = ingest_new_download(&engine, &record, "Cool Game", None).unwrap();
    assert!(!record.artifact.path.exists());
    register(&engine, "g1", "Cool Game");
    register(&engine, "g2", "COOL GAME");
    register(&engine, "g3", "Different Game");
    assert_eq!(
        list_for_game(&engine.db, "g1").unwrap(),
        vec![entry.clone()]
    );
    assert!(matches!(
        verify_for_install(&engine, "g1", &entry.id),
        Err(AppError::Conflict(_))
    ));
    assert!(bind_entry(&engine, "g3", &entry.id).is_err());
    let entry = bind_entry(&engine, "g2", &entry.id).unwrap();
    assert_eq!(entry.game_id.as_deref(), Some("g2"));
    assert_eq!(bind_entry(&engine, "g2", &entry.id).unwrap(), entry);
    assert!(bind_entry(&engine, "g1", &entry.id).is_err());
    assert!(list_for_game(&engine.db, "g1").unwrap().is_empty());
    let (archive, payload) = locations(&engine, &entry).unwrap();
    fs::remove_dir(&payload).unwrap();
    let verified = for_install_path(&engine, "g2", &archive).unwrap().unwrap();
    assert_eq!(verified.payload_path, payload);
    assert_eq!(verified.origin.unwrap().project_id, "demo-project");
    assert!(for_install_path(&engine, "g1", &archive).is_err());
    assert!(for_install_path(&engine, "g2", &archive.with_file_name("Unknown.zip")).is_err());
    fs::write(archive, b"tampered bytes").unwrap();
    assert!(matches!(
        verify_for_install(&engine, "g2", &entry.id),
        Err(AppError::Conflict(_))
    ));
}

#[test]
fn native_move_survives_catalog_commit_failure_and_retries_after_restart() {
    let (_tmp, engine) = setup();
    register(&engine, "g1", "Cool Game");
    let original = catalog(&engine, "Archive.zip", b"complete transport");
    engine
        .db
        .conn()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_publish BEFORE UPDATE OF state ON mod_library_entries
         BEGIN SELECT RAISE(ABORT,'injected publish error'); END;",
        )
        .unwrap();
    assert!(ingest_new_download(&engine, &original, "Cool Game", Some("g1")).is_err());
    assert!(!original.artifact.path.exists());
    assert!(list_for_game(&engine.db, "g1").unwrap().is_empty());
    assert_eq!(
        artifacts::for_id(&engine.db, &original.artifact.id).unwrap(),
        Some(original.clone())
    );
    let paths = (*engine.paths).clone();
    drop(engine);
    let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
    let first = ingest_new_download(&engine, &original, "Cool Game", Some("g1")).unwrap();
    let archive = engine.paths.root.join(&first.archive_relative_path);
    let stored = artifacts::for_path(&engine.db, &archive).unwrap().unwrap();
    assert_eq!(stored.origin(), original.origin());
    assert!(artifacts::for_path(&engine.db, &original.artifact.path)
        .unwrap()
        .is_none());
    assert_eq!(
        ingest_new_download(&engine, &original, "Cool Game", Some("g1")).unwrap(),
        first
    );
    assert_eq!(
        ingest_new_download(&engine, &stored, "Cool Game", Some("g1")).unwrap(),
        first
    );
    assert_eq!(fs::read(archive).unwrap(), b"complete transport");
}

#[test]
fn stale_catalog_and_tampered_source_cannot_publish_but_original_contents_can_retry() {
    let (_tmp, engine) = setup();
    register(&engine, "g1", "Cool Game");
    let record = catalog(&engine, "Archive.zip", b"good");
    let mut forged = record.clone();
    forged.version_id = "other".into();
    assert!(ingest_download(&engine, &forged, "Cool Game", Some("g1")).is_err());
    fs::write(&record.artifact.path, b"evil").unwrap();
    assert!(ingest_download(&engine, &record, "Cool Game", Some("g1")).is_err());
    assert!(list_for_game(&engine.db, "g1").unwrap().is_empty());
    fs::write(&record.artifact.path, b"good").unwrap();
    let entry = ingest_download(&engine, &record, "Cool Game", Some("g1")).unwrap();
    assert_eq!(
        fs::read(engine.paths.root.join(&entry.archive_relative_path)).unwrap(),
        b"good"
    );
    assert_eq!(fs::read(record.artifact.path).unwrap(), b"good");
}

#[test]
fn desktop_receipt_accepts_only_its_managed_namespace_and_never_claims_origin() {
    let (tmp, engine) = setup();
    register(&engine, "g1", "Cool Game");
    let id = uuid::Uuid::new_v4().to_string();
    let filename = "Local Archive.zip";
    let selected = tmp.path().join(filename);
    fs::write(&selected, b"inert fixture").unwrap();
    let hash = hex::encode(Sha256::digest(b"inert fixture"));
    let receipt = ApprovedArtifact {
        artifact_id: &id,
        path: &selected,
        filename,
        size: 13,
        sha256: &hash,
    };
    assert!(ingest_import(&engine, &receipt, "Cool Game", Some("g1")).is_err());
    let dir = engine.paths.staging().join("imports").join(&id);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(filename);
    fs::copy(&selected, &path).unwrap();
    let receipt = ApprovedArtifact {
        path: &path,
        ..receipt
    };
    let entry = ingest_import(&engine, &receipt, "Cool Game", Some("g1")).unwrap();
    fs::remove_file(&path).unwrap();
    assert_eq!(
        ingest_import(&engine, &receipt, "Cool Game", Some("g1")).unwrap(),
        entry
    );
    let verified = verify_for_install(&engine, "g1", &entry.id).unwrap();
    assert!(verified.origin.is_none());
    assert_eq!(verified.entry.source_id, "local");
    assert_eq!(fs::read(&selected).unwrap(), b"inert fixture");
}

#[test]
fn native_metadata_survives_restart_even_if_no_library_row_was_reserved() {
    let (_tmp, engine) = setup();
    register(&engine, "g1", "Cool Game");
    register(&engine, "g2", "Cool Game");
    let (artifact, _) = catalog_fixture(&engine, "Web.zip", b"native web archive");
    let intent = NativeDownloadIntent::new(
        "Cool Game",
        Some("g1"),
        DownloadMetadata {
            project_page: Some("https://example.invalid/the-project"),
            author_hint: Some("  Public Author  "),
        },
    )
    .unwrap();
    artifacts::record_web_with_library_intent(&engine, &artifact, &intent).unwrap();
    let original = artifacts::for_id(&engine.db, &artifact.id)
        .unwrap()
        .unwrap();
    assert_eq!(original.native_library_intent(), Some(&intent));
    engine
        .db
        .conn()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_before_reserve BEFORE INSERT ON mod_library_entries
         BEGIN SELECT RAISE(ABORT, 'injected reserve error'); END;",
        )
        .unwrap();
    assert!(retry_native_download(&engine, &original, "Cool Game", Some("g1")).is_err());
    assert!(artifact.path.exists());
    let paths = (*engine.paths).clone();
    drop(engine);
    let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
    let saved = artifacts::for_id(&engine.db, &artifact.id)
        .unwrap()
        .unwrap();
    assert_eq!(saved.native_library_intent(), Some(&intent));
    assert!(matches!(
        retry_native_download(&engine, &saved, "Cool Game", Some("g2")),
        Err(AppError::Conflict(_))
    ));
    let unmarked = catalog(&engine, "Legacy.zip", b"legacy payload");
    assert!(matches!(
        retry_native_download(&engine, &unmarked, "Cool Game", Some("g1")),
        Err(AppError::Denied(_))
    ));
    let entry = retry_native_download(&engine, &saved, "Cool Game", Some("g1")).unwrap();
    assert_eq!(
        entry.project_page.as_deref(),
        Some("https://example.invalid/the-project")
    );
    assert_eq!(entry.author_hint.as_deref(), Some("Public Author"));
    assert!(!artifact.path.exists());
    let current = artifacts::for_id(&engine.db, &artifact.id)
        .unwrap()
        .unwrap();
    assert_eq!(current.native_library_intent(), Some(&intent));
    assert_eq!(
        ingest_new_download_with_metadata(
            &engine,
            &current,
            "Cool Game",
            Some("g1"),
            DownloadMetadata {
                project_page: Some("https://example.invalid/different-project"),
                author_hint: Some("Different Author"),
            }
        )
        .unwrap(),
        entry
    );
    assert_eq!(
        list_for_game(&engine.db, "g1").unwrap(),
        vec![entry.clone()]
    );
    assert_eq!(
        serde_json::to_value(entry).unwrap()["authorHint"],
        "Public Author"
    );
}

#[test]
fn provider_homepage_wins_and_transfer_endpoints_are_not_persisted() {
    let (_tmp, engine) = setup();
    let (artifact, resolved) = catalog_fixture(&engine, "Verified.zip", b"provider download");
    let intent = NativeDownloadIntent::new(
        "Cool Game",
        None,
        DownloadMetadata {
            project_page: Some("https://example.invalid/user-fallback"),
            author_hint: Some("Public Hint"),
        },
    )
    .unwrap();
    artifacts::record_with_library_intent(&engine, &artifact, &resolved, &intent).unwrap();
    let saved = artifacts::for_id(&engine.db, &artifact.id)
        .unwrap()
        .unwrap();
    let entry = retry_native_download(&engine, &saved, "Cool Game", None).unwrap();
    assert_eq!(
        entry.project_page.as_deref(),
        Some("https://example.invalid/project")
    );
    assert_eq!(entry.author_hint.as_deref(), Some("Public Hint"));

    let (artifact, mut resolved) = catalog_fixture(&engine, "NoHomepage.zip", b"no project page");
    resolved.homepage_url = Some("https://example.invalid/file-without-extension".into());
    resolved.download_url =
        "https://example.invalid/file-without-extension?signature=private-key".into();
    let intent = NativeDownloadIntent::new(
        "Cool Game",
        None,
        DownloadMetadata {
            project_page: resolved.homepage_url.as_deref(),
            author_hint: None,
        },
    )
    .unwrap();
    artifacts::record_with_library_intent(&engine, &artifact, &resolved, &intent).unwrap();
    let saved = artifacts::for_id(&engine.db, &artifact.id)
        .unwrap()
        .unwrap();
    assert!(saved.project_page.is_none());
    assert!(saved
        .native_library_intent()
        .unwrap()
        .project_page
        .is_none());
    let entry = retry_native_download(&engine, &saved, "Cool Game", None).unwrap();
    assert!(entry.project_page.is_none());
    let json: String = engine
        .db
        .conn()
        .query_row(
            "SELECT artifact_json FROM download_artifacts WHERE id=?1",
            [&artifact.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!json.contains("private-key") && !json.contains("file-without-extension"));
    for page in [
        "https://cdn.modrinth.com/data/abc/versions/def/mod.jar",
        "https://example.invalid/public/mod%2Ezip",
        "https://example.invalid/project?token=secret",
        "https://example.invalid/project#secret",
        "https://user:secret@example.invalid/project",
        "http://example.invalid/project",
    ] {
        assert!(
            NativeDownloadIntent::new(
                "Cool Game",
                None,
                DownloadMetadata {
                    project_page: Some(page),
                    author_hint: None,
                }
            )
            .is_err(),
            "accepted unsuitable project page: {page}"
        );
    }
}

#[test]
fn older_library_database_keeps_its_page_when_author_hint_column_is_added() {
    let (_tmp, engine) = setup();
    register(&engine, "g1", "Cool Game");
    let saved = catalog(&engine, "Old.zip", b"v14 payload");
    let legacy = ingest_download(&engine, &saved, "Cool Game", Some("g1")).unwrap();
    assert!(legacy.author_hint.is_none());
    engine
        .db
        .conn()
        .execute_batch(
            "ALTER TABLE mod_library_entries DROP COLUMN author_hint; PRAGMA user_version=14;",
        )
        .unwrap();
    let paths = (*engine.paths).clone();
    drop(engine);
    let reopened = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
    assert_eq!(list_for_game(&reopened.db, "g1").unwrap(), vec![legacy]);
}
