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

struct Fixture {
    _temp: tempfile::TempDir,
    engine: FileEngine,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("manager")).unwrap();
        let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
        Self { _temp: temp, engine }
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

    fn installed_flags(&self) -> (String, bool, bool) {
        self.engine
            .db
            .conn()
            .query_row(
                "SELECT p.version,p.enabled,s.enabled
                 FROM plugins p JOIN skills s ON s.name=p.slug WHERE p.slug=?1",
                [SLUG],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
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
    let skill_markdown = source_skill.to_owned();
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
        steam_app_id: package
            .adapter
            .steam_app_id
            .clone()
            .unwrap_or_default(),
        tags: vec![],
        hidden: false,
        adapter_url: format!(
            "/api/v1/manager-plugins/releases/{release_id}/adapter.json"
        ),
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
    fs::create_dir_all(root).unwrap();
    let registration = crate::games::CatalogGameRegistration::new(
        &engine.paths,
        root,
        Some("Bound test game"),
    )
    .unwrap();
    let game = register_catalog_game(
        engine,
        catalog_preview("1.0.0", "31", "Bound test game"),
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

    update_catalog_release(
        engine,
        &candidate,
        catalog_preview("2.0.0", "32", "Fallout 4 Updated"),
    )
    .unwrap();
    assert_eq!(fixture.installed_flags().0, "2.0.0");
    let (bound_slug,): (Option<String>,) = engine
        .db
        .conn()
        .query_row("SELECT adapter_slug FROM games WHERE id=?1", [&game_id], |row| {
            Ok((row.get(0)?,))
        })
        .unwrap();
    assert_eq!(bound_slug.as_deref(), Some(SLUG));

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

    engine.rollback_transaction(&update_id).unwrap();
    assert_eq!(fixture.installed_flags().0, "1.0.0");
    assert_eq!(bytes(engine, "adapter.json"), old_adapter);
    assert_eq!(bytes(engine, "SKILL.md"), old_skill);
    let (bound_slug,): (Option<String>,) = engine
        .db
        .conn()
        .query_row("SELECT adapter_slug FROM games WHERE id=?1", [&game_id], |row| {
            Ok((row.get(0)?,))
        })
        .unwrap();
    assert_eq!(bound_slug.as_deref(), Some(SLUG));
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
    assert_eq!(fixture.installed_flags(), ("2.0.0".into(), true, false));

    crate::skills::set_enabled(engine, SLUG, true).unwrap();
    set_enabled(engine, SLUG, false).unwrap();
    let candidate = fixture.catalog_candidate();
    update_catalog_release(
        engine,
        &candidate,
        catalog_preview("3.0.0", "33", "Fallout 4"),
    )
    .unwrap();
    assert_eq!(fixture.installed_flags(), ("3.0.0".into(), false, true));

    let update_id = fixture.update_transaction();
    engine.rollback_transaction(&update_id).unwrap();
    assert_eq!(fixture.installed_flags(), ("2.0.0".into(), false, true));
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
    let installed_skill = modified.engine.paths.plugins().join("games").join(SLUG).join("SKILL.md");
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
    assert_eq!(stale.installed_flags().0, "2.0.0");
    assert_eq!(bytes(&stale.engine, "adapter.json"), current_adapter);
}
