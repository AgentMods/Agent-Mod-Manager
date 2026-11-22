use super::*;
use crate::{
    agent::tools::ToolRegistry,
    db::Db,
    mcp::{
        tests::{mock_http, MockSecrets},
        McpServers, McpTransport,
    },
    mcp_connections::McpConnections,
    settings::Settings,
    storage::AppPaths,
};
use serde_json::{json, Value};
use std::sync::Arc;

const SLUG: &str = "test-game";
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
    fn reopened(&self) -> FileEngine {
        FileEngine::new(
            Db::open(&self.engine.paths.db_file()).unwrap(),
            (*self.engine.paths).clone(),
        )
    }
    fn draft(&self, version: &str) -> DraftPreview {
        let (adapter, skill) = content(SLUG, version);
        let preview = plan_write_draft(&self.engine, SLUG, &adapter, &skill).unwrap();
        apply(&self.engine, &preview.plan);
        preview
    }
    fn deployed(&self, version: &str) -> DeploymentPreview {
        let (adapter, skill) = content(SLUG, version);
        self.install_content(SLUG, &adapter, &skill)
    }
    fn install_content(&self, slug: &str, adapter: &str, skill: &str) -> DeploymentPreview {
        let draft = plan_write_draft(&self.engine, slug, adapter, skill).unwrap();
        apply(&self.engine, &draft.plan);
        let deployment = plan_install_plugin(&self.engine, &draft.draft.id).unwrap();
        apply(&self.engine, &deployment.plan);
        reconcile_deployments(&self.engine).unwrap();
        deployment
    }
    fn flags(&self) -> (String, bool, bool, bool) {
        self.engine.db.conn().query_row("SELECT p.version,p.enabled,p.validated,s.enabled FROM plugins p JOIN skills s ON s.name=p.slug WHERE p.slug=?1",[SLUG],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap()
    }
    fn count(&self, table: &str) -> usize {
        self.engine
            .db
            .conn()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
}
fn content(slug: &str, version: &str) -> (String, String) {
    let mut adapter: serde_json::Value =
        serde_json::from_str(plugins::test_fixtures::PACKAGES[0].1).unwrap();
    adapter["slug"] = json!(slug);
    adapter["name"] = json!("Test Game");
    adapter["version"] = json!(version);
    (serde_json::to_string_pretty(&adapter).unwrap(),format!("---\nname: {slug}\ndescription: Declarative test package\n---\n\nVersion {version}. Read only declared registered roots.\n"))
}
fn content_with_mcp(slug: &str, version: &str, servers: Value) -> (String, String) {
    let (adapter, skill) = content(slug, version);
    let mut adapter: Value = serde_json::from_str(&adapter).unwrap();
    adapter["mcpServers"] = servers;
    (serde_json::to_string_pretty(&adapter).unwrap(), skill)
}
fn deploy_package(f: &Fixture, slug: &str, version: &str, mcp_servers: Value) -> DeploymentPreview {
    let (adapter, skill) = content_with_mcp(slug, version, mcp_servers);
    f.install_content(slug, &adapter, &skill)
}
fn owned_mcp_servers(f: &Fixture, slug: &str) -> Vec<crate::mcp::ServerConfig> {
    McpServers::new(f.engine.db.clone())
        .list()
        .unwrap()
        .into_iter()
        .filter(|server| server.plugin_slug.as_deref() == Some(slug))
        .collect()
}

fn apply(engine: &FileEngine, plan: &FilePlan) {
    engine.approve_plan(&plan.id).unwrap();
    engine.apply_file_ops(&plan.id).unwrap();
}
fn write(path: &Path, content: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}
fn legacy(f: &Fixture) -> PathBuf {
    let target = install_path(&f.engine, SLUG).unwrap();
    let (adapter, skill) = content(SLUG, "0.5.0");
    write(&target.join("adapter.json"), &adapter);
    write(&target.join("SKILL.md"), &skill);
    f.engine.db.conn().execute("INSERT INTO plugins(slug,name,game_name,version,description,source,install_path,adapter_json,created_at,updated_at) VALUES (?1,'Test','Test','0.5.0','Declarative test package','external',?2,?3,'original','original')",params![SLUG,target.to_string_lossy(),adapter]).unwrap();
    f.engine.db.conn().execute("INSERT INTO skills(name,description,location,source,created_at) VALUES (?1,'Declarative test package',?2,'external','original')",params![SLUG,target.join("SKILL.md").to_string_lossy()]).unwrap();
    target
}

#[test]
fn draft_writes_are_reviewed_and_discoverable_only_after_persistent_commit() {
    let f = Fixture::new();
    let (adapter, skill) = content(SLUG, "1.0.0");
    let draft = plan_write_draft(&f.engine, SLUG, &adapter, &skill).unwrap();
    assert!(!draft.draft.path.exists());
    assert!(draft.plan.requires_review);
    assert_eq!(draft.plan.entries.len(), 2);
    assert!(draft.plan.entries.iter().all(|e|matches!(&e.operation,FileOp::Copy{src,dst} if src.starts_with(f.engine.paths.staging().join("plugin-scratch")) && dst.starts_with(&draft.draft.path))));
    assert!(list_drafts(&f.engine).unwrap().is_empty());
    assert!(!validate_draft(&f.engine, &draft.draft.id).unwrap().valid);
    assert!(f.engine.apply_file_ops(&draft.plan.id).is_err());
    apply(&f.engine, &draft.plan);
    let reopened = f.reopened();
    assert_eq!(list_drafts(&reopened).unwrap()[0].id, draft.draft.id);
    let validation = validate_draft(&reopened, &draft.draft.id).unwrap();
    assert!(validation.valid, "{:?}", validation.issues);
    assert!(validation.package.is_some() && validation.hashes.is_some());
    assert_eq!(f.count("plugins"), 0);
    assert_eq!(f.count("skills"), 0);
}

#[test]
fn invalid_content_is_saved_but_structurally_explained_and_never_installable() {
    let f = Fixture::new();
    let (adapter, skill) = content(SLUG, "1.0.0");
    let bad = [
        ("not JSON".into(), skill.clone()),
        (
            adapter.clone(),
            "---\nname: wrong-name\ndescription: Test\n---\n".into(),
        ),
        content(SLUG, "not-a-version"),
        content("another-game", "1.0.0"),
        (
            adapter,
            "---\nname: test-game\ndescription: Test\nrun: script\n---\n".into(),
        ),
    ];
    for (adapter, skill) in bad {
        let draft = plan_write_draft(&f.engine, SLUG, &adapter, &skill).unwrap();
        apply(&f.engine, &draft.plan);
        let report = validate_draft(&f.engine, &draft.draft.id).unwrap();
        assert!(!report.valid && !report.issues.is_empty());
        assert!(plan_install_plugin(&f.engine, &draft.draft.id).is_err());
    }
    assert_eq!(list_drafts(&f.engine).unwrap().len(), 5);
    assert_eq!(f.count("plugin_deployments"), 0);
}

#[test]
fn unsafe_slugs_system_skills_and_other_skill_owners_are_rejected() {
    let f = Fixture::new();
    for slug in [
        "../escape",
        "TestGame",
        "con",
        "",
        "system-adapter-authoring",
    ] {
        let (adapter, skill) = content(slug, "1.0.0");
        assert!(plan_write_draft(&f.engine, slug, &adapter, &skill).is_err());
    }
    let (adapter, skill) = content(SLUG, "1.0.0");
    f.engine.db.conn().execute("INSERT INTO skills(name,description,location,source,created_at) VALUES (?1,'manual skill','somewhere','local','now')",[SLUG]).unwrap();
    assert!(plan_write_draft(&f.engine, SLUG, &adapter, &skill).is_err());
    assert_eq!(f.count("file_transactions"), 0);
    assert_eq!(f.count("plugin_drafts"), 0);
}

#[test]
fn reviewed_large_adapter_and_skill_install_reopen_and_upgrade_rollback() {
    let f = Fixture::new();
    let (adapter, skill) = content(SLUG, "1.0.0");
    let mut adapter: Value = serde_json::from_str(&adapter).unwrap();
    adapter["uninstallNotes"] = json!(["Notes 说明".repeat(32 * 1024)]);
    let adapter = adapter.to_string();
    let skill = format!("{skill}{}", "安装说明\n".repeat(96 * 1024));
    let draft = plan_write_draft(&f.engine, SLUG, &adapter, &skill).unwrap();
    assert!(draft.plan.requires_review);
    assert!(f.engine.apply_file_ops(&draft.plan.id).is_err());
    apply(&f.engine, &draft.plan);
    assert!(validate_draft(&f.engine, &draft.draft.id).unwrap().valid);
    let deployment = plan_install_plugin(&f.engine, &draft.draft.id).unwrap();
    apply(&f.engine, &deployment.plan);
    let engine = f.reopened();
    reconcile_deployments(&engine).unwrap();
    assert_eq!(
        plugins::adapter_for(&engine.db, SLUG).unwrap().version,
        "1.0.0"
    );
    assert_eq!(
        crate::skills::get_skill(&engine, SLUG).unwrap().markdown,
        skill
    );
    let upgrade = f.deployed("2.0.0");
    engine.rollback_transaction(&upgrade.plan.id).unwrap();
    reconcile_deployments(&engine).unwrap();
    let path = install_path(&engine, SLUG).unwrap();
    assert_eq!(
        fs::read_to_string(path.join("adapter.json")).unwrap(),
        adapter
    );
    assert_eq!(fs::read_to_string(path.join("SKILL.md")).unwrap(), skill);
    assert_eq!(
        plugins::adapter_for(&engine.db, SLUG).unwrap().version,
        "1.0.0"
    );
}

#[test]
fn modified_draft_and_added_executable_are_rejected_without_running_content() {
    let f = Fixture::new();
    let draft = f.draft("1.0.0");
    let (adapter, _) = content(SLUG, "2.0.0");
    write(&draft.draft.path.join("adapter.json"), adapter);
    let report = validate_draft(&f.engine, &draft.draft.id).unwrap();
    assert!(!report.valid);
    assert!(report.issues[0].message.contains("哈希"));
    assert!(plan_install_plugin(&f.engine, &draft.draft.id).is_err());
    let draft = f.draft("1.0.0");
    write(&draft.draft.path.join("install.sh"), "touch never-run");
    assert!(!validate_draft(&f.engine, &draft.draft.id).unwrap().valid);
    assert!(draft.draft.path.join("install.sh").exists());
    assert!(!draft.draft.path.join("never-run").exists());
}

#[test]
fn draft_path_records_cannot_escape_or_alias_other_drafts() {
    let f = Fixture::new();
    let draft = f.draft("1.0.0");
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE plugin_drafts SET path=?1 WHERE id=?2",
            params![
                f.engine
                    .paths
                    .root
                    .parent()
                    .unwrap()
                    .join("outside")
                    .to_string_lossy(),
                draft.draft.id
            ],
        )
        .unwrap();
    assert!(!validate_draft(&f.engine, &draft.draft.id).unwrap().valid);
    assert!(list_drafts(&f.engine).is_err());
    assert!(plan_install_plugin(&f.engine, &draft.draft.id).is_err());
}

#[test]
fn rolled_back_draft_is_not_installable_or_listed() {
    let f = Fixture::new();
    let draft = f.draft("1.0.0");
    f.engine.rollback_transaction(&draft.plan.id).unwrap();
    assert!(list_drafts(&f.engine).unwrap().is_empty());
    assert!(!validate_draft(&f.engine, &draft.draft.id).unwrap().valid);
    assert!(plan_install_plugin(&f.engine, &draft.draft.id).is_err());
    assert_eq!(f.count("plugin_drafts"), 1);
}

#[test]
fn deployment_is_exactly_two_reviewed_copies_and_recovers_registration_after_restart() {
    let f = Fixture::new();
    let draft = f.draft("1.0.0");
    let preview = plan_install_plugin(&f.engine, &draft.draft.id).unwrap();
    assert_eq!(preview.plan.entries.len(), 2);
    assert!(preview.plan.requires_review);
    assert!(f.engine.get_plan(&preview.plan.id).unwrap().requires_review);
    assert!(!preview.install_path.exists());
    assert!(f.engine.apply_file_ops(&preview.plan.id).is_err());
    assert_eq!(f.count("plugin_deployments"), 1);
    assert_eq!(f.count("plugins"), 0);
    apply(&f.engine, &preview.plan);
    assert_eq!(f.count("plugins"), 0);
    let restarted = f.reopened();
    let report = reconcile_deployments(&restarted).unwrap();
    assert_eq!(report.registered, vec![SLUG]);
    assert_eq!(f.flags(), ("1.0.0".into(), true, true, true));
    assert_eq!(fs::read_dir(&preview.install_path).unwrap().count(), 2);
    assert_eq!(
        plugins::adapter_for(&restarted.db, SLUG).unwrap().version,
        "1.0.0"
    );
    assert_eq!(
        f.engine
            .db
            .conn()
            .query_row("SELECT source FROM plugins WHERE slug=?1", [SLUG], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        SOURCE
    );
}

#[test]
fn pending_or_approved_deployment_never_registers_new_content() {
    let f = Fixture::new();
    let draft = f.draft("1.0.0");
    let preview = plan_install_plugin(&f.engine, &draft.draft.id).unwrap();
    reconcile_deployments(&f.engine).unwrap();
    assert_eq!(f.count("plugins"), 0);
    f.engine.approve_plan(&preview.plan.id).unwrap();
    reconcile_deployments(&f.engine).unwrap();
    assert_eq!(f.count("plugins"), 0);
}

#[test]
fn upgrades_keep_journal_backups_and_rollback_restores_old_registry_or_disables() {
    let f = Fixture::new();
    let first = f.deployed("1.0.0");
    let adapter = fs::read(first.install_path.join("adapter.json")).unwrap();
    let skill = fs::read(first.install_path.join("SKILL.md")).unwrap();
    let second = f.deployed("2.0.0");
    assert_eq!(f.flags().0, "2.0.0");
    assert_eq!(
        fs::read(
            f.engine
                .paths
                .backup_dir(&second.plan.id)
                .join("0.original")
        )
        .unwrap(),
        adapter
    );
    assert_eq!(
        fs::read(
            f.engine
                .paths
                .backup_dir(&second.plan.id)
                .join("1.original")
        )
        .unwrap(),
        skill
    );
    f.engine.rollback_transaction(&second.plan.id).unwrap();
    reconcile_deployments(&f.reopened()).unwrap();
    assert_eq!(f.flags(), ("1.0.0".into(), true, true, true));
    f.engine.rollback_transaction(&first.plan.id).unwrap();
    let report = reconcile_deployments(&f.engine).unwrap();
    assert_eq!(report.disabled, vec![SLUG]);
    assert_eq!(f.flags(), ("1.0.0".into(), false, false, false));
    assert_eq!(f.count("plugin_deployments"), 2);
    assert_eq!(f.count("plugin_drafts"), 2);
}

#[test]
fn pending_upgrade_leaves_active_committed_version_and_user_disable_flags_intact() {
    let f = Fixture::new();
    f.deployed("1.0.0");
    f.engine
        .db
        .conn()
        .execute("UPDATE plugins SET enabled=0 WHERE slug=?1", [SLUG])
        .unwrap();
    f.engine
        .db
        .conn()
        .execute("UPDATE skills SET enabled=0 WHERE name=?1", [SLUG])
        .unwrap();
    let created: String = f
        .engine
        .db
        .conn()
        .query_row(
            "SELECT created_at FROM plugins WHERE slug=?1",
            [SLUG],
            |r| r.get(0),
        )
        .unwrap();
    let draft = f.draft("2.0.0");
    let preview = plan_install_plugin(&f.engine, &draft.draft.id).unwrap();
    reconcile_deployments(&f.engine).unwrap();
    assert_eq!(f.flags(), ("1.0.0".into(), false, true, false));
    apply(&f.engine, &preview.plan);
    reconcile_deployments(&f.engine).unwrap();
    assert_eq!(f.flags(), ("2.0.0".into(), false, true, false));
    assert_eq!(
        f.engine
            .db
            .conn()
            .query_row(
                "SELECT created_at FROM plugins WHERE slug=?1",
                [SLUG],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        created
    );
}

#[test]
fn preexisting_external_baseline_survives_pending_update_and_rollback() {
    let f = Fixture::new();
    legacy(&f);
    let draft = f.draft("1.0.0");
    let preview = plan_install_plugin(&f.engine, &draft.draft.id).unwrap();
    reconcile_deployments(&f.reopened()).unwrap();
    assert_eq!(f.flags(), ("0.5.0".into(), true, true, true));
    apply(&f.engine, &preview.plan);
    reconcile_deployments(&f.engine).unwrap();
    assert_eq!(f.flags().0, "1.0.0");
    f.engine.rollback_transaction(&preview.plan.id).unwrap();
    reconcile_deployments(&f.reopened()).unwrap();
    assert_eq!(f.flags(), ("0.5.0".into(), true, true, true));
}

#[test]
fn unknown_existing_files_are_preserved_and_not_taken_over() {
    let f = Fixture::new();
    let target = install_path(&f.engine, SLUG).unwrap();
    write(&target.join("unknown.txt"), "preserve me");
    let draft = f.draft("1.0.0");
    assert!(plan_install_plugin(&f.engine, &draft.draft.id).is_err());
    assert_eq!(
        fs::read_to_string(target.join("unknown.txt")).unwrap(),
        "preserve me"
    );
    assert_eq!(f.count("plugin_deployments"), 0);
}

#[test]
fn unrelated_external_skill_location_is_not_replaced_or_disabled() {
    let f = Fixture::new();
    let draft = f.draft("1.0.0");
    f.engine.db.conn().execute("INSERT INTO skills(name,description,location,source,created_at) VALUES (?1,'other','unrelated','external','now')",[SLUG]).unwrap();
    assert!(plan_install_plugin(&f.engine, &draft.draft.id).is_err());
    assert_eq!(
        f.engine
            .db
            .conn()
            .query_row(
                "SELECT location,enabled FROM skills WHERE name=?1",
                [SLUG],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
            )
            .unwrap(),
        ("unrelated".into(), true)
    );
}

#[test]
fn disk_hash_mismatch_disables_without_changing_files_and_verified_restore_reenables() {
    let f = Fixture::new();
    let deployment = f.deployed("1.0.0");
    let original = fs::read(deployment.install_path.join("SKILL.md")).unwrap();
    write(
        &deployment.install_path.join("SKILL.md"),
        String::from_utf8(original.clone()).unwrap() + "tampered\n",
    );
    let changed = fs::read(deployment.install_path.join("SKILL.md")).unwrap();
    reconcile_deployments(&f.engine).unwrap();
    assert_eq!(f.flags(), ("1.0.0".into(), false, false, false));
    assert_eq!(
        fs::read(deployment.install_path.join("SKILL.md")).unwrap(),
        changed
    );
    write(&deployment.install_path.join("SKILL.md"), original);
    reconcile_deployments(&f.engine).unwrap();
    assert_eq!(f.flags(), ("1.0.0".into(), true, true, true));
}

#[test]
fn committed_install_remains_valid_after_its_original_draft_is_rolled_back() {
    let f = Fixture::new();
    let draft = f.draft("1.0.0");
    let deployed = plan_install_plugin(&f.engine, &draft.draft.id).unwrap();
    apply(&f.engine, &deployed.plan);
    f.engine.rollback_transaction(&draft.plan.id).unwrap();
    reconcile_deployments(&f.reopened()).unwrap();
    assert_eq!(f.flags(), ("1.0.0".into(), true, true, true));
}

#[test]
fn out_of_order_rollback_preserves_the_current_deployment_and_allows_retry() {
    let f = Fixture::new();
    let first = f.deployed("1.0.0");
    let second = f.deployed("2.0.0");
    assert!(f.engine.rollback_transaction(&first.plan.id).is_err());
    reconcile_deployments(&f.engine).unwrap();
    assert_eq!(f.flags(), ("2.0.0".into(), true, true, true));
    f.engine.rollback_transaction(&second.plan.id).unwrap();
    f.engine.rollback_transaction(&first.plan.id).unwrap();
    reconcile_deployments(&f.engine).unwrap();
    assert!(!f.flags().1 && !f.flags().2 && !f.flags().3);
}

#[test]
fn intermediate_recovery_state_never_exposes_a_partially_applied_package() {
    let f = Fixture::new();
    let deployed = f.deployed("1.0.0");
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE file_transactions SET status='recovery_failed' WHERE id=?1",
            [&deployed.plan.id],
        )
        .unwrap();
    let report = reconcile_deployments(&f.engine).unwrap();
    assert!(report.issues.iter().any(|i| i.code == "recovery_required"));
    assert!(!f.flags().2);
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE file_transactions SET status='committed' WHERE id=?1",
            [&deployed.plan.id],
        )
        .unwrap();
    reconcile_deployments(&f.engine).unwrap();
    assert!(f.flags().1 && f.flags().2 && f.flags().3);
}

#[test]
fn uninstall_by_file_transaction_disables_and_rollback_restores_registration() {
    let f = Fixture::new();
    let deployed = f.deployed("1.0.0");
    let parking = f.engine.paths.staging().join("explicitly-parked");
    let plan = f
        .engine
        .plan_file_ops(
            ["adapter.json", "SKILL.md"]
                .iter()
                .map(|name| FileOp::Move {
                    src: deployed.install_path.join(name),
                    dst: parking.join(name),
                })
                .collect(),
            "Explicit removal".into(),
        )
        .unwrap();
    apply(&f.engine, &plan);
    reconcile_deployments(&f.reopened()).unwrap();
    assert!(!f.flags().2);
    assert_eq!(f.count("plugin_deployments"), 1);
    f.engine.rollback_transaction(&plan.id).unwrap();
    reconcile_deployments(&f.reopened()).unwrap();
    assert!(f.flags().2);
}

#[test]
fn recorded_content_is_reparsed_and_plan_hashes_are_rechecked_during_replay() {
    let f = Fixture::new();
    let deployed = f.deployed("1.0.0");
    let original: String = f
        .engine
        .db
        .conn()
        .query_row(
            "SELECT package_json FROM plugin_deployments WHERE transaction_id=?1",
            [&deployed.plan.id],
            |r| r.get(0),
        )
        .unwrap();
    let mut record: DeploymentRecord = serde_json::from_str(&original).unwrap();
    let (adapter, skill) = content(SLUG, "2.0.0");
    record.content = Content::new(adapter, skill);
    write(
        &deployed.install_path.join("adapter.json"),
        &record.content.adapter_json,
    );
    write(
        &deployed.install_path.join("SKILL.md"),
        &record.content.skill_markdown,
    );
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE plugin_deployments SET package_json=?1 WHERE transaction_id=?2",
            params![serde_json::to_string(&record).unwrap(), deployed.plan.id],
        )
        .unwrap();
    let report = reconcile_deployments(&f.engine).unwrap();
    assert!(!f.flags().2);
    assert!(report.issues.iter().any(|i| i.code == "invalid_record"));
    let (adapter, skill) = content(SLUG, "invalid");
    record.content = Content::new(adapter, skill);
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE plugin_deployments SET package_json=?1 WHERE transaction_id=?2",
            params![serde_json::to_string(&record).unwrap(), deployed.plan.id],
        )
        .unwrap();
    assert!(!reconcile_deployments(&f.engine)
        .unwrap()
        .registered
        .contains(&SLUG.to_string()));
}

#[cfg(unix)]
#[test]
fn symlink_scratch_drafts_and_installed_packages_are_denied_without_following() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = f.engine.paths.root.parent().unwrap().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, f.engine.paths.staging().join("plugin-scratch")).unwrap();
    let (adapter, skill) = content(SLUG, "1.0.0");
    assert!(plan_write_draft(&f.engine, SLUG, &adapter, &skill).is_err());
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    fs::remove_file(f.engine.paths.staging().join("plugin-scratch")).unwrap();
    let draft = f.draft("1.0.0");
    fs::rename(draft.draft.path.join("SKILL.md"), outside.join("SKILL.md")).unwrap();
    symlink(outside.join("SKILL.md"), draft.draft.path.join("SKILL.md")).unwrap();
    assert!(!validate_draft(&f.engine, &draft.draft.id).unwrap().valid);
    let deployed = f.deployed("1.0.0");
    let parked = f.engine.paths.staging().join("old-plugin");
    fs::rename(&deployed.install_path, &parked).unwrap();
    symlink(&outside, &deployed.install_path).unwrap();
    reconcile_deployments(&f.engine).unwrap();
    assert!(!f.flags().2);
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
}

#[test]
fn stale_approved_source_or_destination_is_rejected_by_file_engine() {
    let f = Fixture::new();
    let draft = f.draft("1.0.0");
    let preview = plan_install_plugin(&f.engine, &draft.draft.id).unwrap();
    f.engine.approve_plan(&preview.plan.id).unwrap();
    write(&draft.draft.path.join("SKILL.md"), "changed after preview");
    assert!(f.engine.apply_file_ops(&preview.plan.id).is_err());
    assert!(!preview.install_path.exists());
    assert_eq!(f.count("plugins"), 0);
}

#[test]
fn plugin_toggle_preserves_independent_skill_preference_and_survives_reconciliation() {
    let f = Fixture::new();
    f.deployed("1.0.0");
    assert!(!set_enabled(&f.engine, SLUG, false).unwrap().enabled);
    assert!(plugins::adapter_for(&f.engine.db, SLUG).is_err());
    assert!(f.flags().3);
    reconcile_deployments(&f.engine).unwrap();
    assert!(!f.flags().1);
    f.engine
        .db
        .conn()
        .execute("UPDATE skills SET enabled=0 WHERE name=?1", [SLUG])
        .unwrap();
    assert!(set_enabled(&f.engine, SLUG, true).unwrap().enabled);
    assert!(!f.flags().3);
    assert!(plugins::adapter_for(&f.engine.db, SLUG).is_ok());
    assert_eq!(f.count("file_transactions"), 2);
}

#[test]
fn former_adapter_slugs_install_and_read_as_external_packages() {
    let f = Fixture::new();
    for (slug, adapter, markdown) in plugins::test_fixtures::PACKAGES {
        let deployment = f.install_content(slug, adapter, markdown);
        let info = registered_plugin(&f.engine, slug).unwrap();
        assert_eq!(info.source, "external");
        assert!(info.validated && info.enabled);
        let baseline = read_plugin_package(&f.engine, slug).unwrap();
        assert_eq!(baseline.adapter_json, adapter);
        assert_eq!(baseline.skill_markdown, markdown);
        let detail = crate::skills::get_skill(&f.engine, slug).unwrap();
        assert!(detail.skill.valid);
        assert_eq!(detail.markdown, markdown);
        assert_eq!(
            deployment.install_path,
            f.engine.paths.plugins().join("games").join(slug)
        );
    }
}

#[test]
fn former_adapter_slug_can_be_updated_in_place_and_rejects_undeclared_files() {
    let f = Fixture::new();
    let slug = plugins::test_fixtures::PACKAGES[0].0;
    let first = deploy_package(&f, slug, "1.0.0", json!([]));
    set_enabled(&f.engine, slug, false).unwrap();
    let second = deploy_package(&f, slug, "2.0.0", json!([]));
    assert_eq!(first.install_path, second.install_path);
    let reopened = f.reopened();
    let updated = read_plugin_package(&reopened, slug).unwrap();
    assert_eq!(updated.version, "2.0.0");
    assert!(!updated.enabled);
    assert!(set_enabled(&reopened, slug, true).unwrap().enabled);
    let extra = second
        .install_path
        .join(".previous_0123456789abcdef0123456789abcdef");
    write(&extra, "undeclared file");
    assert!(read_plugin_package(&reopened, slug).is_err());
    assert!(crate::skills::get_skill(&reopened, slug).is_err());
    assert!(set_enabled(&reopened, slug, true).is_err());
    assert_eq!(fs::read_to_string(extra).unwrap(), "undeclared file");
}

#[test]
fn unverified_external_and_unknown_source_plugins_cannot_be_enabled() {
    let f = Fixture::new();
    legacy(&f);
    assert!(set_enabled(&f.engine, SLUG, true).is_err());
    assert!(!set_enabled(&f.engine, SLUG, false).unwrap().enabled);
    f.engine
        .db
        .conn()
        .execute("UPDATE plugins SET source='unknown' WHERE slug=?1", [SLUG])
        .unwrap();
    assert!(set_enabled(&f.engine, SLUG, false).is_err());
    assert!(set_enabled(&f.engine, "../escape", true).is_err());
    assert!(set_enabled(&f.engine, "absent-game", true).is_err());
}

#[test]
fn changed_external_package_cannot_be_reenabled_by_metadata_toggle() {
    let f = Fixture::new();
    let deployed = f.deployed("1.0.0");
    set_enabled(&f.engine, SLUG, false).unwrap();
    write(
        &deployed.install_path.join("SKILL.md"),
        content(SLUG, "2.0.0").1,
    );
    assert!(set_enabled(&f.engine, SLUG, true).is_err());
    assert!(!f.flags().1 && !f.flags().2);
    assert!(!set_enabled(&f.engine, SLUG, false).unwrap().enabled);
}

#[test]
fn toggle_cannot_validate_registry_version_or_unrecoverable_deployment() {
    let f = Fixture::new();
    let deployed = f.deployed("1.0.0");
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE file_transactions SET status='recovery_failed' WHERE id=?1",
            [&deployed.plan.id],
        )
        .unwrap();
    assert!(set_enabled(&f.engine, SLUG, true).is_err());
    assert!(!f.flags().1 && !f.flags().2);
}

#[test]
fn deletion_moves_only_the_selected_draft_and_restore_revives_its_original_id() {
    let f = Fixture::new();
    let engine = &f.engine;
    let chosen = f.draft("1.0.0").draft;
    let sibling = f.draft("2.0.0").draft;
    let deployment = plan_install_plugin(engine, &chosen.id).unwrap();
    apply(engine, &deployment.plan);
    reconcile_deployments(engine).unwrap();
    set_enabled(engine, SLUG, false).unwrap();
    let installed = f.flags();
    let read_adapter = |directory: &Path| fs::read(directory.join("adapter.json")).unwrap();
    let chosen_bytes = read_adapter(&chosen.path);
    let sibling_bytes = read_adapter(&sibling.path);

    delete_confirmed_draft(engine, &chosen.id).unwrap();
    let reopened = f.reopened();
    assert_eq!(list_drafts(&reopened).unwrap()[0].id, sibling.id);
    assert_eq!(list_drafts(&reopened).unwrap().len(), 1);
    assert!(!chosen.path.exists());
    assert_eq!(read_adapter(&sibling.path), sibling_bytes);
    assert_eq!(read_adapter(&deployment.install_path), chosen_bytes);
    assert!(reconcile_deployments(&reopened).unwrap().issues.is_empty());
    assert_eq!(f.flags(), installed);
    assert_eq!(
        get_draft(engine, &chosen.id).unwrap().write_transaction_id,
        chosen.write_transaction_id
    );
    assert_eq!(
        transaction_status(engine, &chosen.write_transaction_id).unwrap(),
        "committed"
    );
    assert!(validate_draft(engine, &chosen.id).is_err());
    assert!(plan_install_plugin(engine, &chosen.id).is_err());
    assert!(delete_confirmed_draft(engine, &chosen.id).is_err());

    let items = trash::list_trash(&engine.db, false).unwrap();
    let [item] = items.as_slice() else {
        panic!("expected one trash item")
    };
    assert_eq!(Path::new(&item.original_path), chosen.path);
    assert_eq!(read_adapter(Path::new(&item.trash_path)), chosen_bytes);
    let move_plan = engine.get_plan(&item.transaction_id).unwrap();
    assert!(move_plan.requires_review && move_plan.entries.len() == 1);
    assert!(
        matches!(&move_plan.entries[0].operation, FileOp::Move { src, .. } if *src == chosen.path)
    );

    let restore = trash::plan_restore(&reopened, std::slice::from_ref(&item.id), false).unwrap();
    apply(&reopened, &restore);
    let validation = validate_draft(engine, &chosen.id).unwrap();
    assert!(validation.valid);
    assert_eq!(
        validation.draft.write_transaction_id,
        chosen.write_transaction_id
    );
    assert!(trash::list_trash(&engine.db, false).unwrap().is_empty());
    assert_eq!(list_drafts(engine).unwrap().len(), 2);

    // Undoing a restore puts the same payload back in the bin, and undoing the
    // original delete in History revives it too (without rewriting provenance).
    engine.rollback_transaction(&restore.id).unwrap();
    assert!(validate_draft(engine, &chosen.id).is_err());
    engine.rollback_transaction(&move_plan.id).unwrap();
    assert!(validate_draft(engine, &chosen.id).unwrap().valid);
}

#[test]
fn deletion_rejects_uncommitted_bad_ids_and_mismatched_source_provenance() {
    let f = Fixture::new();
    let engine = &f.engine;
    let (adapter, skill) = content(SLUG, "1.0.0");
    let planned = plan_write_draft(engine, SLUG, &adapter, &skill)
        .unwrap()
        .draft;
    let chosen = f.draft("1.0.0").draft;
    let other = f.draft("1.0.0").draft;
    assert!(matches!(
        delete_confirmed_draft(engine, "../other"),
        Err(AppError::Invalid(_))
    ));
    assert!(matches!(
        delete_confirmed_draft(engine, &util::new_id("draft")),
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        delete_confirmed_draft(engine, &planned.id),
        Err(AppError::Conflict(_))
    ));
    engine
        .db
        .conn()
        .execute(
            "UPDATE plugin_drafts SET path=?2 WHERE id=?1",
            params![chosen.id, other.path.to_string_lossy()],
        )
        .unwrap();
    assert!(matches!(
        delete_confirmed_draft(engine, &chosen.id),
        Err(AppError::Denied(_))
    ));
    engine
        .db
        .conn()
        .execute(
            "UPDATE plugin_drafts SET path=?2,write_transaction_id=?3 WHERE id=?1",
            params![
                chosen.id,
                chosen.path.to_string_lossy(),
                other.write_transaction_id
            ],
        )
        .unwrap();
    assert!(matches!(
        delete_confirmed_draft(engine, &chosen.id),
        Err(AppError::Invalid(_))
    ));
    assert_eq!(f.count("plugin_draft_deletions"), 0);
    assert!(chosen.path.exists() && other.path.exists());
    assert!(trash::list_trash(&engine.db, false).unwrap().is_empty());
}

#[test]
fn failed_draft_deletion_keeps_draft_live_and_only_committed_moves_appear_in_trash() {
    let f = Fixture::new();
    let engine = &f.engine;
    let draft = f.draft("1.0.0").draft;
    engine
        .db
        .conn()
        .execute_batch(
            "CREATE TRIGGER fail_tombstone BEFORE INSERT ON plugin_draft_deletions
         BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )
        .unwrap();
    assert!(matches!(
        delete_confirmed_draft(engine, &draft.id),
        Err(AppError::Db(_))
    ));
    assert_eq!(f.count("file_transactions"), 1);
    assert_eq!(f.count("trash_items"), 0);
    engine
        .db
        .conn()
        .execute_batch("DROP TRIGGER fail_tombstone")
        .unwrap();
    engine.inject_failure_before(0);
    assert!(delete_confirmed_draft(engine, &draft.id).is_err());
    let reopened = f.reopened();
    assert!(validate_draft(&reopened, &draft.id).unwrap().valid);
    assert_eq!(list_drafts(&reopened).unwrap()[0].id, draft.id);
    assert!(trash::list_trash(&reopened.db, false).unwrap().is_empty());
    delete_confirmed_draft(&reopened, &draft.id).unwrap();
    assert!(list_drafts(&reopened).unwrap().is_empty());
    assert_eq!(trash::list_trash(&reopened.db, false).unwrap().len(), 1);
}

#[test]
fn agent_draft_cleanup_requires_exact_completed_use_and_respects_restore() {
    let f = Fixture::new();
    let engine = &f.engine;
    let manual = f.draft("1.0.0").draft;
    let agent = f.draft("1.0.0").draft;
    let unused = f.draft("1.0.0").draft;
    mark_agent_draft(engine, &agent.id).unwrap();
    mark_agent_draft(engine, &unused.id).unwrap();
    let install = plan_install_plugin(engine, &agent.id).unwrap();
    engine.approve_plan(&install.plan.id).unwrap();
    cleanup_consumed_agent_drafts(engine, None);
    assert_eq!(list_drafts(engine).unwrap().len(), 3);
    engine.apply_file_ops(&install.plan.id).unwrap();

    // Restart between the durable install and cleanup. No chat history is needed.
    let restarted = f.reopened();
    reconcile_deployments(&restarted).unwrap();
    cleanup_consumed_agent_drafts(&restarted, None);
    cleanup_consumed_agent_drafts(&restarted, None);
    assert!(!agent.path.exists());
    assert!(manual.path.exists() && unused.path.exists());
    assert_eq!(list_drafts(engine).unwrap().len(), 2);
    assert_eq!(read_plugin_package(engine, SLUG).unwrap().version, "1.0.0");
    let items = trash::list_trash(&engine.db, false).unwrap();
    assert_eq!(items.len(), 1);
    let restore = trash::plan_restore(engine, std::slice::from_ref(&items[0].id), false).unwrap();
    apply(engine, &restore);
    cleanup_consumed_agent_drafts(engine, None);
    assert!(validate_draft(engine, &agent.id).unwrap().valid);
}

#[test]
fn agent_draft_cleanup_preserves_modified_files_and_pending_consumers() {
    let f = Fixture::new();
    let engine = &f.engine;
    let draft = f.draft("1.0.0").draft;
    mark_agent_draft(engine, &draft.id).unwrap();
    let install = plan_install_plugin(engine, &draft.id).unwrap();
    let duplicate = plan_install_plugin(engine, &draft.id).unwrap();
    apply(engine, &install.plan);
    reconcile_after_apply(engine, &install.plan).unwrap();
    assert!(draft.path.exists());

    // The other reviewed plan can still run after undoing the first install.
    engine.rollback_transaction(&install.plan.id).unwrap();
    apply(engine, &duplicate.plan);
    // The copies completed, but someone added a file before cleanup.
    write(&draft.path.join("my-notes.txt"), "keep me");
    reconcile_after_apply(engine, &duplicate.plan).unwrap();
    assert!(draft.path.join("my-notes.txt").exists());
    assert!(trash::list_trash(&engine.db, false).unwrap().is_empty());
}

#[test]
fn agent_draft_cleanup_migration_only_trusts_matching_success_receipts() {
    let f = Fixture::new();
    let authored = f.draft("1.0.0").draft;
    let imported = f.draft("1.0.0").draft;
    let manual = f.draft("1.0.0").draft;
    let receipt = |id: &str, plan: &str| {
        json!({"status":"committed", "preview":{"draft":{"id":id}}, "plan":{"id":plan}}).to_string()
    };
    for (name, result) in [
        ("write_plugin_draft", receipt(&authored.id, &authored.write_transaction_id)),
        ("import_cloud_plugin", json!({"status":"committed","draftId":imported.id,"plan":{"id":imported.write_transaction_id}}).to_string()),
        ("write_plugin_draft", receipt(&manual.id, &authored.write_transaction_id)),
        ("write_plugin_draft", "old non-JSON receipt".into()),
    ] {
        f.engine.db.conn().execute("INSERT INTO tool_calls(id,conversation_id,name,status,result_json,started_at) VALUES (?1,'chat',?2,'done',?3,'now')", params![util::new_id("tool"),name,result]).unwrap();
    }
    f.engine
        .db
        .conn()
        .pragma_update(None, "user_version", 15)
        .unwrap();
    let reopened = f.reopened();
    let marked: Vec<String> = reopened
        .db
        .conn()
        .prepare("SELECT draft_id FROM agent_plugin_drafts")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(marked.len(), 2);
    assert!(marked.contains(&authored.id) && marked.contains(&imported.id));
}

#[test]
fn reviewed_install_cannot_use_out_of_band_recreated_files_of_a_deleted_draft() {
    let f = Fixture::new();
    let engine = &f.engine;
    let draft = f.draft("1.0.0").draft;
    let deployment = plan_install_plugin(engine, &draft.id).unwrap();
    engine.approve_plan(&deployment.plan.id).unwrap();
    delete_confirmed_draft(engine, &draft.id).unwrap();
    let (adapter, skill) = content(SLUG, "1.0.0");
    write(&draft.path.join("adapter.json"), adapter);
    write(&draft.path.join("SKILL.md"), skill);
    assert!(matches!(
        engine.apply_file_ops(&deployment.plan.id),
        Err(AppError::Conflict(_))
    ));
    assert!(!deployment.install_path.exists());
}

#[test]
fn plugin_mcp_servers_are_inert_until_install_commit_and_support_both_transports() {
    let f = Fixture::new();
    let declared = json!([
        {
            "id":"lookup",
            "name":"Lookup",
            "transport":"streamable_http",
            "url":"https://example.com/mcp"
        },
        {
            "id":"local-helper",
            "name":"Local helper",
            "transport":"stdio",
            "command":"node",
            "args":["server.js","--quiet"]
        }
    ]);
    let (adapter, skill) = content_with_mcp(SLUG, "1.0.0", declared);
    let draft = plan_write_draft(&f.engine, SLUG, &adapter, &skill).unwrap();

    assert!(owned_mcp_servers(&f, SLUG).is_empty());
    apply(&f.engine, &draft.plan);
    assert!(validate_draft(&f.engine, &draft.draft.id).unwrap().valid);
    assert!(owned_mcp_servers(&f, SLUG).is_empty());

    let deployment = plan_install_plugin(&f.engine, &draft.draft.id).unwrap();
    assert!(owned_mcp_servers(&f, SLUG).is_empty());
    apply(&f.engine, &deployment.plan);
    reconcile_deployments(&f.engine).unwrap();

    let mut servers = owned_mcp_servers(&f, SLUG);
    assert_eq!(servers.len(), 2);
    assert!(servers.iter().all(|server| !server.enabled));
    assert!(servers.iter().all(|server| !server.builtin));
    assert!(servers
        .iter()
        .all(|server| server.plugin_slug.as_deref() == Some(SLUG)));
    servers.sort_by(|left, right| left.name.cmp(&right.name));
    let helper = &servers[0];
    assert_eq!(helper.transport, McpTransport::Stdio);
    assert_eq!(helper.command.as_deref(), Some("node"));
    assert_eq!(
        helper.args,
        vec!["server.js".to_string(), "--quiet".to_string()]
    );
    let lookup = &servers[1];
    assert_eq!(lookup.transport, McpTransport::StreamableHttp);
    assert_eq!(lookup.url.as_deref(), Some("https://example.com/mcp"));
}

#[test]
fn plugin_mcp_ownership_conflict_revokes_the_old_declaration() {
    let f = Fixture::new();
    deploy_package(
        &f,
        SLUG,
        "1.0.0",
        json!([{
            "id": "catalog", "name": "Catalog", "transport": "stdio", "command": "catalog-mcp"
        }]),
    );
    let id = owned_mcp_servers(&f, SLUG).pop().unwrap().id;
    let servers = McpServers::new(f.engine.db.clone());
    servers.set_enabled(&id, true).unwrap();
    f.engine
        .db
        .conn()
        .execute("UPDATE skills SET source='user' WHERE name=?1", [SLUG])
        .unwrap();
    let report = reconcile_deployments(&f.engine).unwrap();
    assert!(report.issues.iter().any(|issue| issue.code == "ownership"));
    assert!(matches!(servers.get(&id), Err(AppError::NotFound(_))));
}

#[test]
fn plugin_mcp_local_ids_are_unique_across_owners() {
    let f = Fixture::new();
    let shared_id = |url: &str| {
        json!([{
            "id":"lookup",
            "name":"Lookup",
            "transport":"streamable_http",
            "url":url
        }])
    };
    deploy_package(
        &f,
        "first-game",
        "1.0.0",
        shared_id("https://first.example/mcp"),
    );
    deploy_package(
        &f,
        "second-game",
        "1.0.0",
        shared_id("https://second.example/mcp"),
    );

    let first = owned_mcp_servers(&f, "first-game");
    let second = owned_mcp_servers(&f, "second-game");
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_ne!(first[0].id, second[0].id);
    assert_eq!(first[0].plugin_slug.as_deref(), Some("first-game"));
    assert_eq!(second[0].plugin_slug.as_deref(), Some("second-game"));
    assert_eq!(first[0].url.as_deref(), Some("https://first.example/mcp"));
    assert_eq!(second[0].url.as_deref(), Some("https://second.example/mcp"));
}

#[tokio::test]
async fn plugin_mcp_upgrade_disable_reenable_removal_and_rollback_stay_owner_scoped() {
    let f = Fixture::new();
    legacy(&f); // An older package has no mcpServers declaration.
    assert!(owned_mcp_servers(&f, SLUG).is_empty());

    let (url, stop_server, _) = mock_http("echo").await;
    let v1_servers = json!([{
        "id":"lookup",
        "name":"Lookup",
        "transport":"streamable_http",
        "url":url
    }]);
    let first = deploy_package(&f, SLUG, "1.0.0", v1_servers);
    let servers = McpServers::new(f.engine.db.clone());
    let initial = owned_mcp_servers(&f, SLUG);
    assert_eq!(initial.len(), 1);
    let server_id = initial[0].id.clone();
    assert!(!initial[0].enabled);

    // A successful, explicit connection is the only operation that grants startup restore.
    let settings =
        Settings::with_secret_store(f.engine.db.clone(), Arc::new(MockSecrets::default()));
    let connections = McpConnections::default();
    let registry = ToolRegistry::default();
    let mut expected = servers.get(&server_id).unwrap();
    expected.enabled = true;
    connections
        .connect(settings.clone(), &registry, expected, false)
        .await
        .unwrap();
    assert!(servers.get(&server_id).unwrap().enabled);
    assert_eq!(servers.startup_requests().unwrap(), vec![server_id.clone()]);
    drop((connections, registry));

    let upgraded_url = format!("{url}/upgraded");
    let second = deploy_package(
        &f,
        SLUG,
        "2.0.0",
        json!([{
            "id":"lookup",
            "name":"Lookup v2",
            "transport":"streamable_http",
            "url":upgraded_url
        }]),
    );
    let upgraded = owned_mcp_servers(&f, SLUG);
    assert_eq!(upgraded.len(), 1);
    assert_eq!(upgraded[0].id, server_id);
    assert_eq!(upgraded[0].url.as_deref(), Some(upgraded_url.as_str()));
    assert!(servers.startup_requests().unwrap().is_empty());

    f.engine.rollback_transaction(&second.plan.id).unwrap();
    reconcile_deployments(&f.engine).unwrap();
    let restored = owned_mcp_servers(&f, SLUG);
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].url.as_deref(), Some(url.as_str()));
    assert!(servers.startup_requests().unwrap().is_empty());

    // Recreate a remembered start after rollback, then ensure plugin disable revokes it.
    let connections = McpConnections::default();
    let registry = ToolRegistry::default();
    let mut expected = servers.get(&server_id).unwrap();
    expected.enabled = true;
    connections
        .connect(settings.clone(), &registry, expected, false)
        .await
        .unwrap();
    assert_eq!(servers.startup_requests().unwrap(), vec![server_id.clone()]);
    drop((connections, registry));

    assert!(!set_enabled(&f.engine, SLUG, false).unwrap().enabled);
    assert!(owned_mcp_servers(&f, SLUG).is_empty());
    assert!(!servers.get(&server_id).unwrap().enabled);
    assert!(servers.startup_requests().unwrap().is_empty());
    assert!(servers.set_enabled(&server_id, true).is_err());

    // A disabled owner rejects a connect request even when the UI marks it enabled.
    let connections = McpConnections::default();
    let registry = ToolRegistry::default();
    connections
        .restore_requested(&settings, &registry, || {})
        .await;
    assert!(connections.list().await.is_empty());
    let mut expected = servers.get(&server_id).unwrap();
    expected.enabled = true;
    assert!(matches!(
        connections
            .connect(settings.clone(), &registry, expected, false)
            .await,
        Err(AppError::Denied(_))
    ));
    assert!(connections.list().await.is_empty());

    assert!(set_enabled(&f.engine, SLUG, true).unwrap().enabled);
    let reenabled = owned_mcp_servers(&f, SLUG);
    assert_eq!(reenabled.len(), 1);
    assert!(!reenabled[0].enabled);
    assert!(servers.startup_requests().unwrap().is_empty());
    connections
        .restore_requested(&settings, &registry, || {})
        .await;
    assert!(connections.list().await.is_empty());

    // Re-enabling the plugin does not reconnect it; an explicit connect starts it manually.
    let mut expected = servers.get(&server_id).unwrap();
    expected.enabled = true;
    connections
        .connect(settings.clone(), &registry, expected, false)
        .await
        .unwrap();
    assert!(servers.get(&server_id).unwrap().enabled);
    assert_eq!(connections.list().await.len(), 1);
    connections
        .disconnect(&registry, &servers, &server_id)
        .await
        .unwrap();

    let parked = f.engine.paths.staging().join("removed-plugin-package");
    let removal = f
        .engine
        .plan_file_ops(
            ["adapter.json", "SKILL.md"]
                .iter()
                .map(|name| FileOp::Move {
                    src: first.install_path.join(name),
                    dst: parked.join(name),
                })
                .collect(),
            "Remove plugin package".into(),
        )
        .unwrap();
    apply(&f.engine, &removal);
    reconcile_deployments(&f.engine).unwrap();
    assert!(owned_mcp_servers(&f, SLUG).is_empty());
    assert!(servers.get(&server_id).is_err());

    f.engine.rollback_transaction(&removal.id).unwrap();
    reconcile_deployments(&f.engine).unwrap();
    let after_remove_rollback = owned_mcp_servers(&f, SLUG);
    assert_eq!(after_remove_rollback.len(), 1);
    assert_eq!(after_remove_rollback[0].url.as_deref(), Some(url.as_str()));
    assert!(servers.startup_requests().unwrap().is_empty());

    // Rolling the first MCP-capable deployment back restores the pre-existing legacy package.
    f.engine.rollback_transaction(&first.plan.id).unwrap();
    reconcile_deployments(&f.engine).unwrap();
    assert!(owned_mcp_servers(&f, SLUG).is_empty());
    assert!(servers.startup_requests().unwrap().is_empty());
    stop_server.cancel();
}
