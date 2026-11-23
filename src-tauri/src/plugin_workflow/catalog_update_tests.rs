//! Desktop catalog update lifecycle regression tests.
use super::*;
use crate::{
    db::Db,
    manager_catalog::{CatalogPlugin, CatalogPreview},
    storage::AppPaths,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

const SLUG: &str = "fallout4";
const PLUGIN_ID: &str = "9007199254740993";
const CATALOG_SOURCE: &str = "https://www.glossmod.com/manager-plugins";

#[derive(Debug, PartialEq, Eq)]
struct RegistrySnapshot {
    version: String,
    plugin_enabled: bool,
    validated: bool,
    adapter_json: String,
    skill_location: String,
    skill_enabled: bool,
}

struct Fixture {
    _temp: tempfile::TempDir,
    engine: FileEngine,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("manager")).unwrap();
        let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
        Self {
            _temp: temp,
            engine,
        }
    }

    fn catalog_candidate(&self) -> CatalogUpdateCandidate {
        let candidates = catalog_update_candidates(&self.engine).unwrap();
        candidates
            .into_iter()
            .find(|candidate| candidate.slug == SLUG)
            .expect("installed cloud release should have an update candidate")
    }

    fn update_transaction(&self) -> String {
        self.engine
            .db
            .conn()
            .query_row(
                "SELECT d.transaction_id
                 FROM plugin_deployments d
                 JOIN file_transactions f ON f.id=d.transaction_id
                 WHERE d.slug=?1 AND f.kind='catalog_plugin_update' AND f.status='committed'
                 ORDER BY d.rowid DESC LIMIT 1",
                [SLUG],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn registry(&self) -> RegistrySnapshot {
        self.engine
            .db
            .conn()
            .query_row(
                "SELECT p.version,p.enabled,p.validated,p.adapter_json,s.location,s.enabled
                 FROM plugins p JOIN skills s ON s.name=p.slug WHERE p.slug=?1",
                [SLUG],
                |row| {
                    Ok(RegistrySnapshot {
                        version: row.get(0)?,
                        plugin_enabled: row.get(1)?,
                        validated: row.get(2)?,
                        adapter_json: row.get(3)?,
                        skill_location: row.get(4)?,
                        skill_enabled: row.get(5)?,
                    })
                },
            )
            .unwrap()
    }

    fn game_binding(&self, game_id: &str) -> (Option<String>, Option<String>) {
        self.engine
            .db
            .conn()
            .query_row(
                "SELECT adapter_slug,adapter_version FROM games WHERE id=?1",
                [game_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }
}

fn catalog_preview(version: &str, release_id: &str, game_name: &str) -> CatalogPreview {
    let (_, source_adapter, source_skill) = crate::plugins::test_fixtures::PACKAGES[2];
    let mut adapter: Value = serde_json::from_str(source_adapter).unwrap();
    adapter["version"] = json!(version);
    adapter["gameName"] = json!(game_name);
    let adapter_json = serde_json::to_string_pretty(&adapter).unwrap();
    let skill_markdown = format!("{source_skill}\nRelease {version}.\n");
    let package = crate::plugins::parse_package(SLUG, &adapter_json, &skill_markdown).unwrap();
    let plugin = CatalogPlugin {
        id: PLUGIN_ID.into(),
        release_id: release_id.into(),
        slug: SLUG.into(),
        name: package.adapter.name.clone(),
        game_name: Some(game_name.into()),
        version: version.into(),
        platforms: package.adapter.platforms.clone(),
        description: package.skill.description.clone(),
        author_id: "7".into(),
        author_name: "Fixture author".into(),
        steam_app_id: package.adapter.steam_app_id.clone().unwrap_or_default(),
        tags: vec![],
        hidden: false,
        adapter_url: format!("/api/v1/manager-plugins/releases/{release_id}/adapter.json"),
        skill_url: format!("/api/v1/manager-plugins/releases/{release_id}/SKILL.md"),
    };
    CatalogPreview {
        game_name: game_name.into(),
        plugin,
        source: CATALOG_SOURCE,
        hashes: PackageHashes::of(&adapter_json, &skill_markdown),
        mcp_servers: package.adapter.mcp_servers.clone(),
        adapter_json,
        skill_markdown,
        installed_version: None,
        display: None,
    }
}

fn install_catalog(engine: &FileEngine, version: &str, release_id: &str, game_name: &str) {
    install_catalog_release(engine, catalog_preview(version, release_id, game_name)).unwrap();
}

fn apply(engine: &FileEngine, plan: &FilePlan) {
    engine.approve_plan(&plan.id).unwrap();
    engine.apply_file_ops(&plan.id).unwrap();
}

fn bytes(engine: &FileEngine, file: &str) -> Vec<u8> {
    fs::read(engine.paths.plugins().join("games").join(SLUG).join(file)).unwrap()
}

fn bind_game(engine: &FileEngine, root: &Path) -> String {
    // Keep this aligned with manager_catalog_tests::fallout_root so explicit
    // registration follows the same marker-based Fallout 4 discovery path.
    fs::create_dir_all(root.join("Data")).unwrap();
    fs::write(root.join("Fallout4.exe"), "fixture").unwrap();
    fs::write(root.join("Data/Fallout4.esm"), "fixture").unwrap();
    let registration =
        crate::games::CatalogGameRegistration::new(&engine.paths, root, Some("Bound test game"))
            .unwrap();
    let game = register_catalog_game(
        engine,
        // Reuse is allowed only for the exact installed release bytes,
        // including the adapter's gameName field.
        catalog_preview("1.0.0", "31", "Fallout 4"),
        registration,
    )
    .unwrap();
    game.id
}

#[test]
fn catalog_update_keeps_existing_game_binding_and_rollback_restores_prior_release() {
    let fixture = Fixture::new();
    let engine = &fixture.engine;
    install_catalog(engine, "1.0.0", "31", "Fallout 4");
    let old_adapter = bytes(engine, "adapter.json");
    let old_skill = bytes(engine, "SKILL.md");
    let game_id = bind_game(engine, &fixture._temp.path().join("game"));
    let candidate = fixture.catalog_candidate();
    assert_eq!(candidate.slug, SLUG);
    assert_eq!(candidate.version, "1.0.0");
    assert_eq!(candidate.plugin_id.as_deref(), Some(PLUGIN_ID));

    let next = catalog_preview("2.0.0", "32", "Fallout 4 Updated");
    let new_adapter = next.adapter_json.as_bytes().to_vec();
    let new_skill = next.skill_markdown.as_bytes().to_vec();
    update_catalog_release(engine, &candidate, next).unwrap();
    assert_eq!(fixture.registry().version, "2.0.0");
    assert_ne!(bytes(engine, "adapter.json"), old_adapter);
    assert_ne!(bytes(engine, "SKILL.md"), old_skill);
    assert_eq!(bytes(engine, "adapter.json"), new_adapter);
    assert_eq!(bytes(engine, "SKILL.md"), new_skill);
    assert_eq!(fixture.game_binding(&game_id).0.as_deref(), Some(SLUG));

    let update_id = fixture.update_transaction();
    let kind: String = engine
        .db
        .conn()
        .query_row(
            "SELECT kind FROM file_transactions WHERE id=?1",
            [&update_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, "catalog_plugin_update");

    assert!(engine.rollback_transaction(&update_id).is_err());
    engine.rollback_desktop_transaction(&update_id).unwrap();
    assert_eq!(fixture.registry().version, "1.0.0");
    assert_eq!(bytes(engine, "adapter.json"), old_adapter);
    assert_eq!(bytes(engine, "SKILL.md"), old_skill);
    assert_eq!(fixture.game_binding(&game_id).0.as_deref(), Some(SLUG));
    let restored = fixture.catalog_candidate();
    assert_eq!(restored.version, "1.0.0");
    assert_eq!(restored.deployment_id, candidate.deployment_id);
}

#[test]
fn catalog_update_preserves_plugin_and_skill_preferences_independently() {
    let fixture = Fixture::new();
    let engine = &fixture.engine;
    install_catalog(engine, "1.0.0", "31", "Fallout 4");

    crate::skills::set_enabled(engine, SLUG, false).unwrap();
    let candidate = fixture.catalog_candidate();
    update_catalog_release(
        engine,
        &candidate,
        catalog_preview("2.0.0", "32", "Fallout 4"),
    )
    .unwrap();
    let registry = fixture.registry();
    assert_eq!(
        (
            registry.version.as_str(),
            registry.plugin_enabled,
            registry.skill_enabled
        ),
        ("2.0.0", true, false)
    );

    crate::skills::set_enabled(engine, SLUG, true).unwrap();
    set_enabled(engine, SLUG, false).unwrap();
    let candidate = fixture.catalog_candidate();
    update_catalog_release(
        engine,
        &candidate,
        catalog_preview("3.0.0", "33", "Fallout 4"),
    )
    .unwrap();
    let registry = fixture.registry();
    assert_eq!(
        (
            registry.version.as_str(),
            registry.plugin_enabled,
            registry.skill_enabled
        ),
        ("3.0.0", false, true)
    );

    let update_id = fixture.update_transaction();
    engine.rollback_desktop_transaction(&update_id).unwrap();
    let registry = fixture.registry();
    assert_eq!(
        (
            registry.version.as_str(),
            registry.plugin_enabled,
            registry.skill_enabled
        ),
        ("2.0.0", false, true)
    );
}

#[test]
fn legacy_catalog_install_without_origin_metadata_uses_unknown_plugin_id_until_update() {
    let fixture = Fixture::new();
    install_catalog(&fixture.engine, "1.0.0", "31", "Fallout 4");
    let installed = fixture.catalog_candidate();
    assert_eq!(installed.plugin_id.as_deref(), Some(PLUGIN_ID));
    assert_eq!(
        fixture
            .engine
            .db
            .conn()
            .execute(
                "DELETE FROM catalog_plugin_origins WHERE transaction_id=?1",
                [&installed.deployment_id],
            )
            .unwrap(),
        1
    );

    let legacy = fixture.catalog_candidate();
    assert_eq!(legacy.version, "1.0.0");
    assert_eq!(legacy.deployment_id, installed.deployment_id);
    assert_eq!(legacy.plugin_id, None);
    update_catalog_release(
        &fixture.engine,
        &legacy,
        catalog_preview("2.0.0", "32", "Fallout 4"),
    )
    .unwrap();

    let updated = fixture.catalog_candidate();
    assert_eq!(updated.version, "2.0.0");
    assert_eq!(updated.plugin_id.as_deref(), Some(PLUGIN_ID));
}

#[test]
fn failed_second_file_copy_restores_release_binding_and_registry_and_can_retry() {
    let fixture = Fixture::new();
    let engine = &fixture.engine;
    install_catalog(engine, "1.0.0", "31", "Fallout 4");
    let game_id = bind_game(engine, &fixture._temp.path().join("game"));
    let candidate = fixture.catalog_candidate();
    let old_adapter = bytes(engine, "adapter.json");
    let old_skill = bytes(engine, "SKILL.md");
    let old_registry = fixture.registry();
    assert_eq!(old_registry.adapter_json.as_bytes(), old_adapter);
    assert_eq!(
        old_registry.skill_location,
        engine
            .paths
            .plugins()
            .join("games")
            .join(SLUG)
            .join("SKILL.md")
            .to_string_lossy()
            .into_owned()
    );
    assert_eq!(
        fixture.game_binding(&game_id),
        (Some(SLUG.into()), Some("1.0.0".into()))
    );

    let next = catalog_preview("2.0.0", "32", "Fallout 4 Updated");
    let new_adapter = next.adapter_json.as_bytes().to_vec();
    let new_skill = next.skill_markdown.as_bytes().to_vec();
    engine.inject_failure_before(1);
    assert!(update_catalog_release(engine, &candidate, next).is_err());

    assert_eq!(bytes(engine, "adapter.json"), old_adapter);
    assert_eq!(bytes(engine, "SKILL.md"), old_skill);
    assert_eq!(fixture.registry(), old_registry);
    assert_eq!(
        fixture.game_binding(&game_id),
        (Some(SLUG.into()), Some("1.0.0".into()))
    );
    assert_eq!(fixture.catalog_candidate(), candidate);

    // The injected failure is intentionally sticky until reset; retry with the
    // same current deployment after clearing the test hook.
    engine.inject_failure_before(usize::MAX);
    update_catalog_release(
        engine,
        &candidate,
        catalog_preview("2.0.0", "32", "Fallout 4 Updated"),
    )
    .unwrap();
    let registry = fixture.registry();
    assert_eq!(registry.version, "2.0.0");
    assert_eq!(registry.adapter_json.as_bytes(), new_adapter);
    assert_eq!(bytes(engine, "adapter.json"), new_adapter);
    assert_eq!(bytes(engine, "SKILL.md"), new_skill);
    assert_ne!(bytes(engine, "adapter.json"), old_adapter);
    assert_ne!(bytes(engine, "SKILL.md"), old_skill);
    assert_eq!(fixture.game_binding(&game_id).0.as_deref(), Some(SLUG));
}

#[test]
fn catalog_update_registry_failure_rolls_back_both_files_without_replaying_failed_insert() {
    let fixture = Fixture::new();
    let engine = &fixture.engine;
    install_catalog(engine, "1.0.0", "31", "Fallout 4");
    let candidate = fixture.catalog_candidate();
    let registry = fixture.registry();
    let old_adapter = bytes(engine, "adapter.json");
    let old_skill = bytes(engine, "SKILL.md");
    engine
        .db
        .conn()
        .execute_batch(
            "CREATE TRIGGER reject_skill BEFORE INSERT ON skills
        BEGIN SELECT RAISE(ABORT, 'injected registry error'); END",
        )
        .unwrap();
    assert!(update_catalog_release(
        engine,
        &candidate,
        catalog_preview("2.0.0", "32", "Fallout 4")
    )
    .is_err());
    let status: String = engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM file_transactions WHERE kind='catalog_plugin_update'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "rolled_back");
    assert_eq!(fixture.registry(), registry);
    assert_eq!(bytes(engine, "adapter.json"), old_adapter);
    assert_eq!(bytes(engine, "SKILL.md"), old_skill);
}

#[test]
fn local_install_modified_content_and_stale_candidates_cannot_be_catalog_updated() {
    let local = Fixture::new();
    let preview = catalog_preview("1.0.0", "31", "Fallout 4");
    let draft = plan_write_draft(
        &local.engine,
        SLUG,
        &preview.adapter_json,
        &preview.skill_markdown,
    )
    .unwrap();
    apply(&local.engine, &draft.plan);
    let deployment = plan_install_plugin(&local.engine, &draft.draft.id).unwrap();
    apply(&local.engine, &deployment.plan);
    reconcile_deployments(&local.engine).unwrap();
    assert!(catalog_update_candidates(&local.engine).unwrap().is_empty());

    let modified = Fixture::new();
    install_catalog(&modified.engine, "1.0.0", "31", "Fallout 4");
    let modified_candidate = modified.catalog_candidate();
    let installed_skill = modified
        .engine
        .paths
        .plugins()
        .join("games")
        .join(SLUG)
        .join("SKILL.md");
    fs::write(&installed_skill, b"user-edited instructions\n").unwrap();
    let user_edit = fs::read(&installed_skill).unwrap();
    assert!(update_catalog_release(
        &modified.engine,
        &modified_candidate,
        catalog_preview("2.0.0", "32", "Fallout 4"),
    )
    .is_err());
    assert_eq!(fs::read(&installed_skill).unwrap(), user_edit);

    let stale = Fixture::new();
    install_catalog(&stale.engine, "1.0.0", "31", "Fallout 4");
    let old_candidate = stale.catalog_candidate();
    update_catalog_release(
        &stale.engine,
        &old_candidate,
        catalog_preview("2.0.0", "32", "Fallout 4"),
    )
    .unwrap();
    let current_adapter = bytes(&stale.engine, "adapter.json");
    assert!(update_catalog_release(
        &stale.engine,
        &old_candidate,
        catalog_preview("3.0.0", "33", "Fallout 4"),
    )
    .is_err());
    assert_eq!(stale.registry().version, "2.0.0");
    assert_eq!(bytes(&stale.engine, "adapter.json"), current_adapter);
}
