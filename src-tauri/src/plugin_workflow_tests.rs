use super::*;
use crate::{db::Db, storage::AppPaths};
use serde_json::json;

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
        let draft = self.draft(version);
        let preview = plan_install_plugin(&self.engine, &draft.draft.id).unwrap();
        apply(&self.engine, &preview.plan);
        reconcile_deployments(&self.engine).unwrap();
        preview
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
        serde_json::from_str(plugins::BUILTIN_PACKAGES[0].1).unwrap();
    adapter["slug"] = json!(slug);
    adapter["name"] = json!("Test Game");
    adapter["version"] = json!(version);
    (serde_json::to_string_pretty(&adapter).unwrap(),format!("---\nname: {slug}\ndescription: Declarative test package\n---\n\nVersion {version}. Read only declared registered roots.\n"))
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
fn builtin_slugs_dynamic_builtin_sources_and_unsafe_slugs_cannot_be_taken_over() {
    let f = Fixture::new();
    for slug in ["minecraft-java", "../escape", "TestGame", "con", ""] {
        let (adapter, skill) = content(slug, "1.0.0");
        assert!(plan_write_draft(&f.engine, slug, &adapter, &skill).is_err());
    }
    let (adapter, skill) = content(SLUG, "1.0.0");
    f.engine.db.conn().execute("INSERT INTO skills(name,description,location,source,created_at) VALUES (?1,'reserved','somewhere','builtin','now')",[SLUG]).unwrap();
    assert!(plan_write_draft(&f.engine, SLUG, &adapter, &skill).is_err());
    assert_eq!(f.count("file_transactions"), 0);
    assert_eq!(f.count("plugin_drafts"), 0);
}

#[test]
fn oversized_inputs_are_rejected_before_any_plan_or_draft_record() {
    let f = Fixture::new();
    assert!(plan_write_draft(&f.engine, SLUG, &"x".repeat(ADAPTER_LIMIT + 1), "skill").is_err());
    assert!(plan_write_draft(&f.engine, SLUG, "{}", &"x".repeat(SKILL_LIMIT + 1)).is_err());
    assert_eq!(f.count("file_transactions"), 0);
    assert_eq!(f.count("plugin_drafts"), 0);
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
fn builtin_toggle_checks_embedded_content_and_preserves_preference_on_reseed() {
    let f = Fixture::new();
    plugins::seed_builtins(&f.engine.db, &f.engine.paths).unwrap();
    let slug = "minecraft-java";
    assert!(!set_enabled(&f.engine, slug, false).unwrap().enabled);
    plugins::seed_builtins(&f.engine.db, &f.engine.paths).unwrap();
    assert!(
        !plugins::list_plugins(&f.engine.db)
            .unwrap()
            .into_iter()
            .find(|p| p.slug == slug)
            .unwrap()
            .enabled
    );
    assert!(set_enabled(&f.engine, slug, true).unwrap().enabled);
    let target = install_path(&f.engine, slug).unwrap();
    write(
        &target.join("SKILL.md"),
        format!(
            "{}\nUnapproved instructions.\n",
            plugins::BUILTIN_PACKAGES[0].2
        ),
    );
    assert!(set_enabled(&f.engine, slug, true).is_err());
    assert!(!set_enabled(&f.engine, slug, false).unwrap().enabled);
}

#[test]
fn builtin_package_and_skill_remain_readable_after_reseed() {
    let f = Fixture::new();
    let (slug, adapter, markdown) = plugins::BUILTIN_PACKAGES[0];
    plugins::seed_builtins(&f.engine.db, &f.engine.paths).unwrap();
    let directory = install_path(&f.engine, slug).unwrap();
    let old_adapter = format!("{adapter}\n");
    write(&directory.join("adapter.json"), &old_adapter);
    plugins::seed_builtins(&f.engine.db, &f.engine.paths).unwrap();

    let backup = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap())
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".previous_")
        })
        .unwrap();
    assert_eq!(fs::read_to_string(backup.path()).unwrap(), old_adapter);
    assert!(validate_package_directory(&f.engine, &directory, false).is_err());

    let baseline = read_plugin_package(&f.engine, slug).unwrap();
    assert_eq!(baseline.adapter_json, adapter);
    assert_eq!(baseline.skill_markdown, markdown);
    let detail = crate::skills::get_skill(&f.engine, slug).unwrap();
    assert!(detail.skill.valid);
    assert_eq!(detail.markdown, markdown);

    write(&directory.join(".previous_extra.exe"), "undeclared asset");
    assert!(matches!(
        read_plugin_package(&f.engine, slug),
        Err(AppError::Denied(_))
    ));
    assert!(matches!(
        crate::skills::get_skill(&f.engine, slug),
        Err(AppError::Denied(_))
    ));
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
