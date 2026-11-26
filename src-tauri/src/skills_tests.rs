use super::*;
use crate::storage::AppPaths;
use std::{sync::mpsc, thread, time::Duration};

const NAME: &str = "load-order-notes";
struct Fixture {
    _temp: tempfile::TempDir,
    engine: FileEngine,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("manager")).unwrap();
        let db = Db::open(&paths.db_file()).unwrap();
        migrate(&db).unwrap();
        Self {
            _temp: temp,
            engine: FileEngine::new(db, paths),
        }
    }
    fn reopen(&self) -> FileEngine {
        let db = Db::open(&self.engine.paths.db_file()).unwrap();
        migrate(&db).unwrap();
        FileEngine::new(db, (*self.engine.paths).clone())
    }
    fn install(&self, version: &str) -> SkillPreview {
        let p = plan_install_skill(&self.engine, NAME, &markdown(NAME, version)).unwrap();
        apply(&self.engine, &p.plan);
        reconcile_deployments(&self.engine).unwrap();
        p
    }
    fn count(&self, table: &str) -> usize {
        self.engine
            .db
            .conn()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
    fn flag(&self) -> bool {
        self.engine
            .db
            .conn()
            .query_row("SELECT enabled FROM skills WHERE name=?1", [NAME], |r| {
                r.get(0)
            })
            .unwrap()
    }
}
fn markdown(name: &str, version: &str) -> String {
    format!("---\nname: {name}\ndescription: Declarative load order notes\n---\n\nRead registered files. Edition {version}.\n")
}
fn apply(engine: &FileEngine, plan: &FilePlan) {
    engine.approve_plan(&plan.id).unwrap();
    engine.apply_file_ops(&plan.id).unwrap();
}

/// Run a Skill read while both global mutation locks are held by this thread.
/// A broken implementation may wait on either lock, so bound the wait and
/// release both locks before allowing that worker to finish.
fn while_skill_mutation_locks_held<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let skills = lock();
    let files = crate::transaction::write_lock();
    let (send, receive) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let _ = send.send(f());
    });

    let result = receive.recv_timeout(Duration::from_secs(2));
    drop(files);
    drop(skills);
    let value = result.expect("Skill operation did not return while both mutation locks were held");
    worker.join().expect("Skill lock probe worker panicked");
    value
}

fn write(path: &Path, content: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}
fn set_status(f: &Fixture, id: &str, status: &str) {
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE file_transactions SET status=?2 WHERE id=?1",
            params![id, status],
        )
        .unwrap();
}
fn insert_skill(f: &Fixture, name: &str, source: &str, path: &Path) {
    f.engine.db.conn().execute("INSERT INTO skills(name,description,location,source,created_at) VALUES (?1,'Existing',?2,?3,'original')",params![name,path.to_string_lossy(),source]).unwrap();
}

fn install_plugin(f: &Fixture) {
    let mut adapter: serde_json::Value =
        serde_json::from_str(plugins::test_fixtures::PACKAGES[0].1).unwrap();
    adapter["slug"] = serde_json::json!(NAME);
    let draft = crate::plugin_workflow::plan_write_draft(
        &f.engine,
        NAME,
        &serde_json::to_string(&adapter).unwrap(),
        &markdown(NAME, "plugin"),
    )
    .unwrap();
    apply(&f.engine, &draft.plan);
    let deployed = crate::plugin_workflow::plan_install_plugin(&f.engine, &draft.draft.id).unwrap();
    apply(&f.engine, &deployed.plan);
    crate::plugin_workflow::reconcile_deployments(&f.engine).unwrap();
}

#[test]
fn game_package_skill_is_readable_but_cannot_be_taken_over_and_tampering_is_detected() {
    let f = Fixture::new();
    install_plugin(&f);
    let detail = get_skill(&f.engine, NAME).unwrap();
    assert_eq!(detail.skill.kind, SkillKind::Plugin);
    assert!(detail.skill.valid);
    assert!(plan_install_skill(&f.engine, NAME, &markdown(NAME, "standalone")).is_err());
    assert!(plan_uninstall_skill(&f.engine, NAME).is_err());
    assert!(!set_enabled(&f.engine, NAME, false).unwrap().enabled);
    assert!(!get_skill(&f.engine, NAME).unwrap().skill.enabled);
    assert!(set_enabled(&f.engine, NAME, true).unwrap().enabled);
    write(
        &detail.skill.location,
        markdown(NAME, "unapproved-plugin-edit"),
    );
    assert!(get_skill(&f.engine, NAME).is_err());
    assert!(set_enabled(&f.engine, NAME, true).is_err());
    assert!(!set_enabled(&f.engine, NAME, false).unwrap().enabled);
    assert_eq!(f.count("skills_deployments"), 0);
}

#[test]
fn settings_skill_listing_and_system_skill_access_do_not_wait_on_mutation_locks() {
    const SYSTEM_NAME: &str = "system-mod-uninstall";

    let f = Fixture::new();
    let installed = f.install("settings-list-lock-probe");
    set_enabled(&f.engine, NAME, false).unwrap();

    let engine = f.engine.clone();
    let listed = while_skill_mutation_locks_held(move || list_settings_skills(&engine));
    let listed = listed.expect("settings Skill listing failed");
    let system = listed
        .iter()
        .find(|skill| skill.name == SYSTEM_NAME)
        .expect("embedded Skill missing from settings list");
    assert_eq!(system.kind, SkillKind::System);
    let manual = listed
        .iter()
        .find(|skill| skill.name == NAME)
        .expect("installed manual Skill missing from settings list");
    assert_eq!(manual.kind, SkillKind::Standalone);
    assert!(manual.valid);
    assert!(
        !manual.enabled,
        "disabled state should be listed as disabled"
    );

    set_enabled(&f.engine, NAME, true).unwrap();
    write(
        &installed.location,
        markdown(NAME, "tampered-settings-list-lock-probe"),
    );
    let engine = f.engine.clone();
    let listed = while_skill_mutation_locks_held(move || list_settings_skills(&engine))
        .expect("settings Skill listing failed after tampering");
    let manual = listed
        .iter()
        .find(|skill| skill.name == NAME)
        .expect("tampered manual Skill should remain visible in settings");
    assert!(!manual.valid, "tampered manual Skill must be unavailable");
    assert!(
        !manual.enabled,
        "tampered Skill must be listed as unavailable"
    );
    assert!(
        f.flag(),
        "listing a tampered Skill must preserve enabled preference"
    );

    let engine = f.engine.clone();
    let (detail, disabled) = while_skill_mutation_locks_held(move || {
        let detail = get_skill(&engine, SYSTEM_NAME)?;
        let disabled = set_enabled(&engine, SYSTEM_NAME, false)?;
        Ok::<_, AppError>((detail, disabled))
    })
    .expect("embedded Skill operations failed");
    assert_eq!(detail.skill.kind, SkillKind::System);
    assert!(detail.skill.valid);
    assert!(!detail.markdown.is_empty());
    assert!(!disabled.enabled);
}

fn assert_effective_plugin_skill_preference(f: &Fixture, name: &str, preference: bool) {
    set_enabled(&f.engine, name, preference).unwrap();
    for plugin_enabled in [false, true, false, true] {
        // Model the plugin toggle's contract: only plugins.enabled is changed.
        f.engine
            .db
            .conn()
            .execute(
                "UPDATE plugins SET enabled=?2 WHERE slug=?1",
                params![name, plugin_enabled],
            )
            .unwrap();
        let reopened = f.reopen();
        let detail = get_skill(&reopened, name).unwrap();
        assert!(detail.skill.valid);
        assert!(!detail.markdown.is_empty());
        assert_eq!(detail.skill.enabled, preference && plugin_enabled);
        let listed = list_skills(&reopened)
            .unwrap()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap();
        assert!(listed.valid);
        assert_eq!(listed.enabled, preference && plugin_enabled);
        let stored: bool = f
            .engine
            .db
            .conn()
            .query_row("SELECT enabled FROM skills WHERE name=?1", [name], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(stored, preference);
    }
}

#[test]
fn external_plugin_toggle_masks_get_and_list_but_preserves_skill_preference() {
    for preference in [true, false] {
        let f = Fixture::new();
        install_plugin(&f);
        assert_effective_plugin_skill_preference(&f, NAME, preference);
    }
}

#[test]
fn uninstalled_game_package_name_is_available_for_a_standalone_skill() {
    let f = Fixture::new();
    let name = plugins::test_fixtures::PACKAGES[0].0;
    assert!(matches!(
        get_skill(&f.engine, name),
        Err(AppError::NotFound(_))
    ));
    let text = markdown(name, "standalone");
    let plan = plan_install_skill(&f.engine, name, &text).unwrap();
    apply(&f.engine, &plan.plan);
    let detail = get_skill(&f.engine, name).unwrap();
    assert_eq!(detail.skill.kind, SkillKind::Standalone);
    assert_eq!(detail.markdown, text);
    assert_eq!(f.count("plugins"), 0);
}

#[test]
fn planned_plugin_deployment_reserves_name_before_it_is_registered() {
    let f = Fixture::new();
    let mut adapter: serde_json::Value =
        serde_json::from_str(plugins::test_fixtures::PACKAGES[0].1).unwrap();
    adapter["slug"] = serde_json::json!(NAME);
    let draft = crate::plugin_workflow::plan_write_draft(
        &f.engine,
        NAME,
        &serde_json::to_string(&adapter).unwrap(),
        &markdown(NAME, "plugin"),
    )
    .unwrap();
    apply(&f.engine, &draft.plan);
    let deployed = crate::plugin_workflow::plan_install_plugin(&f.engine, &draft.draft.id).unwrap();
    assert_eq!(f.count("plugins"), 0);
    assert!(plan_install_skill(&f.engine, NAME, &markdown(NAME, "standalone")).is_err());
    set_status(&f, &deployed.plan.id, "failed");
    assert!(plan_install_skill(&f.engine, NAME, &markdown(NAME, "standalone")).is_ok());
}

#[test]
fn migration_is_additive_and_idempotent() {
    let f = Fixture::new();
    insert_skill(&f, NAME, "unknown", Path::new("arbitrary"));
    migrate(&f.engine.db).unwrap();
    migrate(&f.engine.db).unwrap();
    assert_eq!(f.count("skills"), 1);
    assert_eq!(f.count("skills_deployments"), 0);
}

#[test]
fn embedded_catalog_lists_readable_system_skills_with_persisted_preferences() {
    let f = Fixture::new();
    let guides = [
        ("system-adapter-authoring", "read_plugin_package"),
        ("system-web-search", "web_search"),
        ("system-mod-download", "GameBanana"),
        (
            "system-mod-commission",
            "https://www.glossmod.com/commissions/new",
        ),
        ("system-mod-install", "match_mod_install"),
        (MOD_ORGANIZER_SKILL_NAME, "insufficientEvidence"),
        ("system-mod-uninstall", "inspect_install"),
    ];
    let listed = list_skills(&f.engine).unwrap();
    assert_eq!(listed.len(), guides.len());
    for (name, expected_text) in guides {
        let info = listed.iter().find(|skill| skill.name == name).unwrap();
        assert!(is_system_skill(name));
        assert_eq!(info.kind, SkillKind::System);
        assert_eq!(serde_json::to_value(info).unwrap()["kind"], "system");
        assert!(info.enabled && info.valid && info.issues.is_empty());
        let detail = get_skill(&f.engine, name).unwrap();
        assert_eq!(detail.skill.description, info.description);
        assert!(detail.markdown.contains(expected_text));
    }
    assert!(!is_system_skill(NAME));

    let disabled = set_enabled(&f.engine, "system-web-search", false).unwrap();
    assert!(!disabled.enabled);
    let reopened = f.reopen();
    let listed = list_skills(&reopened).unwrap();
    assert!(listed
        .iter()
        .all(|skill| skill.enabled == (skill.name != disabled.name)));
    let detail = get_skill(&reopened, &disabled.name).unwrap();
    assert!(!detail.skill.enabled);
    assert!(detail.markdown.contains("web_search"));
    assert!(
        set_enabled(&reopened, &disabled.name, true)
            .unwrap()
            .enabled
    );
    assert!(
        get_skill(&f.reopen(), &disabled.name)
            .unwrap()
            .skill
            .enabled
    );
    assert_eq!(f.count("file_transactions"), 0);
    assert_eq!(f.count("skills"), 0);
}

#[test]
fn organizer_worker_reads_the_catalog_body_and_respects_persisted_toggle() {
    let f = Fixture::new();
    let name = MOD_ORGANIZER_SKILL_NAME;
    let detail = get_skill(&f.engine, name).unwrap();
    let body = enabled_mod_organizer(&f.engine.db).unwrap().unwrap();
    assert!(body.starts_with("# 整理已安装的 Mod"));
    assert!(detail.markdown.trim_end().ends_with(body));

    set_enabled(&f.engine, name, false).unwrap();
    let reopened = f.reopen();
    assert_eq!(enabled_mod_organizer(&reopened.db).unwrap(), None);
    assert!(!get_skill(&reopened, name).unwrap().skill.enabled);
    set_enabled(&reopened, name, true).unwrap();
    assert_eq!(enabled_mod_organizer(&f.reopen().db).unwrap(), Some(body));
}

#[tokio::test]
async fn agent_read_skill_uses_catalog_preference_and_marks_content_untrusted() {
    use crate::{
        agent::tools::{ToolContext, ToolRegistry},
        sources::resolutions::ResolutionScope,
    };
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    let f = Fixture::new();
    let tool = ToolRegistry::builtin().unwrap().get("read_skill").unwrap();
    let name = "system-web-search";
    let args = json!({"name":name});
    let context = ToolContext {
        engine: f.engine.clone(),
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: ResolutionScope::Agent("system-skill-test".into()),
    };
    let prepared = tool.prepare(&context, &args).await.unwrap();
    let value = tool
        .execute(&context, args.clone(), prepared)
        .await
        .unwrap();
    assert_eq!(value["name"], name);
    assert_eq!(value["content"], system_skill(name).unwrap().markdown);
    assert_eq!(value["trust"], "untrusted_instructions");

    set_enabled(&f.engine, name, false).unwrap();
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert!(matches!(
        tool.execute(&context, args, prepared).await,
        Err(AppError::Denied(_))
    ));
}

#[test]
fn system_name_rejects_standalone_deployment_and_ignores_legacy_registry_content() {
    let f = Fixture::new();
    let name = "system-adapter-authoring";
    let target = location(&f.engine, name).unwrap();
    let legacy_markdown = markdown(name, "legacy");
    assert!(plan_install_skill(&f.engine, name, &legacy_markdown).is_err());
    assert!(plan_uninstall_skill(&f.engine, name).is_err());

    write(&target, &legacy_markdown);
    insert_skill(&f, name, SOURCE, &target);
    let reopened = f.reopen();
    let listed = list_skills(&reopened).unwrap();
    let matching = listed
        .iter()
        .filter(|skill| skill.name == name)
        .collect::<Vec<_>>();
    assert_eq!(matching.len(), 1);
    assert_eq!(matching[0].kind, SkillKind::System);
    assert_eq!(
        get_skill(&reopened, name).unwrap().markdown,
        system_skill(name).unwrap().markdown
    );
    set_enabled(&reopened, name, false).unwrap();
    let legacy_row = registry_row(&reopened, name).unwrap().unwrap();
    assert!(legacy_row.enabled);
    assert_eq!(legacy_row.source, SOURCE);
    assert_eq!(legacy_row.location, target);
    assert_eq!(fs::read_to_string(target).unwrap(), legacy_markdown);
    assert_eq!(f.count("file_transactions"), 0);
}

#[test]
fn installs_exactly_one_declared_file_only_after_review_and_commit() {
    let f = Fixture::new();
    let text = markdown(NAME, "one");
    let p = plan_install_skill(&f.engine, NAME, &text).unwrap();
    assert!(!p.location.exists());
    assert_eq!(p.plan.entries.len(), 1);
    assert!(p.plan.requires_review);
    assert!(
        matches!(&p.plan.entries[0].operation,FileOp::Copy{src,dst} if src.starts_with(f.engine.paths.staging().join("skill-scratch")) && *dst==p.location)
    );
    assert_eq!(f.count("skills"), 0);
    assert!(f.engine.apply_file_ops(&p.plan.id).is_err());
    apply(&f.engine, &p.plan);
    // Simulate process loss immediately after the file commit, before registry.
    let reopened = f.reopen();
    let report = reconcile_deployments(&reopened).unwrap();
    assert_eq!(report.registered, [NAME]);
    let detail = get_skill(&reopened, NAME).unwrap();
    assert_eq!(detail.markdown, text);
    assert!(detail.skill.enabled && detail.skill.valid);
    assert_eq!(detail.skill.kind, SkillKind::Standalone);
    assert_eq!(
        fs::read_dir(p.location.parent().unwrap()).unwrap().count(),
        1
    );
}

#[test]
fn invalid_name_and_frontmatter_are_rejected_before_plan() {
    let f = Fixture::new();
    for name in ["", "../escape", "C:/escape", "Upper", "a--b", "nul", "a/b"] {
        assert!(plan_install_skill(&f.engine, name, &markdown(name, "one")).is_err());
    }
    for text in [
        "# No frontmatter".to_string(),
        markdown("other-name", "one"),
        format!("---\nname: {NAME}\ndescription: Test\nrun: anything\n---\nBody"),
        format!(
            "---\nname: {NAME}\ndescription: {}\n---\nBody",
            "x".repeat(1025)
        ),
        format!("---\nname: {NAME}\ndescription: Test\ndescription: Duplicate\n---\nBody"),
        format!("---\nname: {NAME}\ndescription: Test\n---\n\0"),
    ] {
        assert!(plan_install_skill(&f.engine, NAME, &text).is_err());
    }
    assert_eq!(f.count("file_transactions"), 0);
    assert_eq!(f.count("skills_deployments"), 0);
}

#[test]
fn large_standalone_skill_install_and_uninstall_rollback_are_replayable() {
    let f = Fixture::new();
    let text = format!(
        "{}{}",
        markdown(NAME, "one"),
        "完整说明\n".repeat(96 * 1024)
    );
    let install = plan_install_skill(&f.engine, NAME, &text).unwrap();
    assert!(install.plan.requires_review);
    assert!(f.engine.apply_file_ops(&install.plan.id).is_err());
    apply(&f.engine, &install.plan);
    reconcile_deployments(&f.engine).unwrap();
    assert_eq!(get_skill(&f.engine, NAME).unwrap().markdown, text);
    let uninstall = plan_uninstall_skill(&f.engine, NAME).unwrap();
    apply(&f.engine, &uninstall.plan);
    reconcile_deployments(&f.engine).unwrap();
    assert!(!install.location.exists());
    f.engine.rollback_transaction(&uninstall.plan.id).unwrap();
    reconcile_deployments(&f.engine).unwrap();
    assert_eq!(get_skill(&f.engine, NAME).unwrap().markdown, text);
}

#[test]
fn unicode_description_limit_and_crlf_are_validated_by_shared_parser() {
    let f = Fixture::new();
    let text = format!(
        "---\r\nname: {NAME}\r\ndescription: {}\r\n---\r\nBody\r\n",
        "\u{6e38}".repeat(1024)
    );
    let p = plan_install_skill(&f.engine, NAME, &text).unwrap();
    apply(&f.engine, &p.plan);
    assert_eq!(get_skill(&f.engine, NAME).unwrap().markdown, text);
}

#[test]
fn installed_package_skills_cannot_be_overwritten_or_removed_as_standalone_skills() {
    let f = Fixture::new();
    plugins::test_fixtures::install(&f.engine.db, &f.engine.paths).unwrap();
    for (name, _, text) in plugins::test_fixtures::PACKAGES {
        assert!(plan_install_skill(&f.engine, name, text).is_err());
        assert!(plan_uninstall_skill(&f.engine, name).is_err());
    }
    f.engine.db.conn().execute("INSERT INTO plugins(slug,name,game_name,version,install_path,adapter_json,source,created_at,updated_at) VALUES (?1,'Test','Test','1.0.0','arbitrary','{}','external','now','now')",[NAME]).unwrap();
    assert!(plan_install_skill(&f.engine, NAME, &markdown(NAME, "one")).is_err());
    assert_eq!(f.count("skills_deployments"), 0);
}

#[test]
fn external_source_alone_never_grants_standalone_ownership() {
    for source in ["builtin", "external", "downloaded", "unknown"] {
        let f = Fixture::new();
        let arbitrary = f.engine.paths.staging().join("elsewhere/SKILL.md");
        write(&arbitrary, markdown(NAME, "secret"));
        insert_skill(&f, NAME, source, &arbitrary);
        assert!(plan_install_skill(&f.engine, NAME, &markdown(NAME, "one")).is_err());
        assert!(get_skill(&f.engine, NAME).is_err());
        assert!(set_enabled(&f.engine, NAME, true).is_err());
        assert!(set_enabled(&f.engine, NAME, false).is_err());
        let info = list_skills(&f.engine)
            .unwrap()
            .into_iter()
            .find(|skill| skill.name == NAME)
            .unwrap();
        assert!(!info.valid);
        assert_eq!(info.kind, SkillKind::Unknown);
        assert!(arbitrary.exists());
    }
}

#[test]
fn unknown_files_and_legacy_external_document_are_not_silently_adopted() {
    let f = Fixture::new();
    let target = location(&f.engine, NAME).unwrap();
    write(&target, markdown(NAME, "legacy"));
    insert_skill(&f, NAME, SOURCE, &target);
    assert!(plan_install_skill(&f.engine, NAME, &markdown(NAME, "one")).is_err());
    assert!(get_skill(&f.engine, NAME).is_err());
    assert!(plan_uninstall_skill(&f.engine, NAME).is_err());
    assert_eq!(f.count("skills_deployments"), 0);
}

#[test]
fn updates_preserve_created_at_and_disabled_preference_through_rollback() {
    let f = Fixture::new();
    f.install("one");
    let created = get_skill(&f.engine, NAME).unwrap().skill.created_at;
    set_enabled(&f.engine, NAME, false).unwrap();
    let second = f.install("two");
    assert!(!f.flag());
    assert_eq!(
        get_skill(&f.engine, NAME).unwrap().skill.created_at,
        created
    );
    f.engine.rollback_transaction(&second.plan.id).unwrap();
    let reopened = f.reopen();
    let detail = get_skill(&reopened, NAME).unwrap();
    assert!(detail.markdown.contains("Edition one"));
    assert!(!detail.skill.enabled);
    set_enabled(&reopened, NAME, true).unwrap();
    assert!(get_skill(&reopened, NAME).unwrap().skill.enabled);
    assert_eq!(f.count("skills_deployments"), 2);
    assert_eq!(f.count("file_transactions"), 2);
}

#[test]
fn committed_uninstall_is_retained_and_rollback_restores_document() {
    let f = Fixture::new();
    let first = f.install("one");
    let removal = plan_uninstall_skill(&f.engine, NAME).unwrap();
    assert_eq!(removal.action, SkillAction::Uninstall);
    assert!(removal.plan.requires_review);
    assert!(first.location.exists());
    let retained = match &removal.plan.entries[0].operation {
        FileOp::Move { src, dst } => {
            assert_eq!(src, &first.location);
            dst.clone()
        }
        _ => panic!("not a retained move"),
    };
    assert!(retained.starts_with(f.engine.paths.trash()));
    assert_eq!(retained.file_name().unwrap(), "payload");
    assert!(retained.parent().unwrap().join("manifest.json").is_file());
    assert!(trash::list_trash(&f.engine.db, false).unwrap().is_empty());
    assert!(f.engine.apply_file_ops(&removal.plan.id).is_err());
    apply(&f.engine, &removal.plan);
    let reopened = f.reopen();
    assert_eq!(reconcile_deployments(&reopened).unwrap().removed, [NAME]);
    assert!(!first.location.exists());
    assert!(retained.exists());
    let items = trash::list_trash(&reopened.db, false).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(Path::new(&items[0].original_path), first.location);
    assert_eq!(items[0].transaction_id, removal.plan.id);
    assert!(!items[0].is_dir);
    assert!(!f.flag());
    assert!(get_skill(&reopened, NAME).is_err());
    assert!(set_enabled(&reopened, NAME, true).is_err());
    f.engine.rollback_transaction(&removal.plan.id).unwrap();
    assert!(get_skill(&reopened, NAME).unwrap().skill.enabled);
    assert_eq!(f.count("skills"), 1);
    assert_eq!(f.count("skills_deployments"), 2);
}

#[test]
fn standard_trash_restore_and_its_rollback_reconcile_skill_state() {
    let f = Fixture::new();
    f.install("one");
    set_enabled(&f.engine, NAME, false).unwrap();
    let removal = plan_uninstall_skill(&f.engine, NAME).unwrap();
    apply(&f.engine, &removal.plan);
    reconcile_deployments(&f.engine).unwrap();
    let item = trash::list_trash(&f.engine.db, false).unwrap().remove(0);
    let restore = trash::plan_restore(&f.engine, &[item.id], false).unwrap();
    apply(&f.engine, &restore);
    let detail = get_skill(&f.reopen(), NAME).unwrap();
    assert!(detail.markdown.contains("Edition one"));
    assert!(!detail.skill.enabled);
    f.engine.rollback_transaction(&restore.id).unwrap();
    assert_eq!(reconcile_deployments(&f.engine).unwrap().removed, [NAME]);
    assert!(get_skill(&f.engine, NAME).is_err());
    assert_eq!(f.count("trash_items"), 1);
    assert_eq!(f.count("skills_deployments"), 2);
}

#[test]
fn uninstall_manifest_db_binding_and_payload_tampering_fail_closed() {
    for mode in [
        "manifest",
        "matching-manifest-and-db",
        "db-target",
        "db-transaction",
        "payload",
    ] {
        let f = Fixture::new();
        f.install("one");
        let removal = plan_uninstall_skill(&f.engine, NAME).unwrap();
        apply(&f.engine, &removal.plan);
        let item = trash::list_trash(&f.engine.db, false).unwrap().remove(0);
        let manifest = Path::new(&item.trash_path)
            .parent()
            .unwrap()
            .join("manifest.json");
        match mode {
            "manifest" => write(&manifest, b"{}"),
            "matching-manifest-and-db" => {
                let mut decoded: trash::TrashItem =
                    serde_json::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
                decoded.reason = "tampered metadata".into();
                write(&manifest, serde_json::to_vec_pretty(&decoded).unwrap());
                f.engine
                    .db
                    .conn()
                    .execute(
                        "UPDATE trash_items SET reason=?1 WHERE id=?2",
                        params![decoded.reason, item.id],
                    )
                    .unwrap();
            }
            "db-target" => {
                f.engine
                    .db
                    .conn()
                    .execute(
                        "UPDATE trash_items SET original_path='/outside/SKILL.md' WHERE id=?1",
                        [item.id],
                    )
                    .unwrap();
            }
            "db-transaction" => {
                f.engine
                    .db
                    .conn()
                    .execute(
                        "UPDATE trash_items SET transaction_id='other' WHERE id=?1",
                        [item.id],
                    )
                    .unwrap();
            }
            "payload" => write(Path::new(&item.trash_path), markdown(NAME, "unapproved")),
            _ => unreachable!(),
        }
        let report = reconcile_deployments(&f.reopen()).unwrap();
        assert!(report.removed.is_empty(), "{mode}");
        assert!(!report.issues.is_empty(), "{mode}");
        assert!(!f.flag(), "{mode}");
        assert!(get_skill(&f.engine, NAME).is_err(), "{mode}");
    }
}

#[test]
fn old_staging_uninstall_records_are_not_accepted_as_standard_trash() {
    let f = Fixture::new();
    f.install("one");
    let removal = plan_uninstall_skill(&f.engine, NAME).unwrap();
    apply(&f.engine, &removal.plan);
    let row = deployment_rows(&f.engine).unwrap().remove(0);
    let mut record = record_for(&f.engine, &row).unwrap();
    record.trash = None;
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE skills_deployments SET record_json=?1 WHERE transaction_id=?2",
            params![serde_json::to_string(&record).unwrap(), removal.plan.id],
        )
        .unwrap();
    assert!(reconcile_deployments(&f.engine).unwrap().removed.is_empty());
    assert!(!f.flag());
    assert!(get_skill(&f.engine, NAME).is_err());
}

#[test]
fn rollback_new_install_disables_but_never_deletes_history() {
    let f = Fixture::new();
    let p = f.install("one");
    f.engine.rollback_transaction(&p.plan.id).unwrap();
    let report = reconcile_deployments(&f.reopen()).unwrap();
    assert_eq!(report.disabled, [NAME]);
    assert!(!f.flag());
    assert!(get_skill(&f.engine, NAME).is_err());
    assert_eq!(f.count("skills"), 1);
    assert_eq!(f.count("skills_deployments"), 1);
    assert_eq!(f.count("file_transactions"), 1);
}

#[test]
fn content_from_only_rolled_back_deployment_cannot_be_resurrected() {
    let f = Fixture::new();
    let p = f.install("one");
    f.engine.rollback_transaction(&p.plan.id).unwrap();
    write(&p.location, markdown(NAME, "one"));
    let report = reconcile_deployments(&f.engine).unwrap();
    assert!(!report.issues.is_empty());
    assert!(!f.flag());
    assert!(get_skill(&f.engine, NAME).is_err());
}

#[test]
fn pending_failed_and_intermediate_transactions_cannot_register_document() {
    for status in [
        "planned",
        "approved",
        "failed",
        "applying",
        "rolling_back",
        "recovery_failed",
    ] {
        let f = Fixture::new();
        let p = plan_install_skill(&f.engine, NAME, &markdown(NAME, "one")).unwrap();
        write(&p.location, markdown(NAME, "one"));
        set_status(&f, &p.plan.id, status);
        let report = reconcile_deployments(&f.engine).unwrap();
        assert!(report.registered.is_empty(), "{status}");
        assert_eq!(f.count("skills"), 0, "{status}");
        assert!(get_skill(&f.engine, NAME).is_err());
    }
}

#[test]
fn incomplete_update_disables_known_skill_until_recovered() {
    let f = Fixture::new();
    f.install("one");
    let p = plan_install_skill(&f.engine, NAME, &markdown(NAME, "two")).unwrap();
    set_status(&f, &p.plan.id, "recovery_failed");
    let report = reconcile_deployments(&f.engine).unwrap();
    assert_eq!(report.disabled, [NAME]);
    assert!(!f.flag());
    assert!(get_skill(&f.engine, NAME).is_err());
    set_status(&f, &p.plan.id, "failed");
    assert!(get_skill(&f.engine, NAME).unwrap().skill.enabled);
}

#[test]
fn tampered_oversize_binary_missing_and_extra_asset_fail_closed() {
    for mode in ["changed", "large-tamper", "binary", "missing", "extra"] {
        let f = Fixture::new();
        let p = f.install("one");
        match mode {
            "changed" => write(&p.location, markdown(NAME, "unapproved")),
            "large-tamper" => write(
                &p.location,
                format!(
                    "{}{}",
                    markdown(NAME, "tampered"),
                    "新行\n".repeat(32 * 1024)
                ),
            ),
            "binary" => write(&p.location, [0xff, 0, 0x80]),
            "missing" => fs::remove_file(&p.location).unwrap(),
            "extra" => write(&p.location.parent().unwrap().join("run.sh"), "do not run"),
            _ => unreachable!(),
        }
        let report = reconcile_deployments(&f.engine).unwrap();
        assert_eq!(report.disabled, [NAME], "{mode}");
        assert!(!f.flag(), "{mode}");
        assert!(get_skill(&f.engine, NAME).is_err(), "{mode}");
        assert!(set_enabled(&f.engine, NAME, true).is_err(), "{mode}");
        assert!(plan_uninstall_skill(&f.engine, NAME).is_err(), "{mode}");
        assert!(
            plan_install_skill(&f.engine, NAME, &markdown(NAME, "replacement")).is_err()
                || mode == "missing"
        );
    }
}

#[test]
fn safety_disable_does_not_override_explicit_preference() {
    let f = Fixture::new();
    let p = f.install("one");
    write(&p.location, markdown(NAME, "changed"));
    reconcile_deployments(&f.engine).unwrap();
    assert!(!f.flag());
    write(&p.location, markdown(NAME, "one"));
    assert!(get_skill(&f.engine, NAME).unwrap().skill.enabled);
    set_enabled(&f.engine, NAME, false).unwrap();
    write(&p.location, markdown(NAME, "changed"));
    reconcile_deployments(&f.engine).unwrap();
    write(&p.location, markdown(NAME, "one"));
    assert!(!get_skill(&f.engine, NAME).unwrap().skill.enabled);
}

#[test]
fn scratch_tamper_is_rejected_by_file_engine_before_commit() {
    let f = Fixture::new();
    let p = plan_install_skill(&f.engine, NAME, &markdown(NAME, "one")).unwrap();
    let src = match &p.plan.entries[0].operation {
        FileOp::Copy { src, .. } => src,
        _ => unreachable!(),
    };
    write(src, markdown(NAME, "tampered"));
    f.engine.approve_plan(&p.plan.id).unwrap();
    assert!(f.engine.apply_file_ops(&p.plan.id).is_err());
    assert!(!p.location.exists());
    assert!(!list_skills(&f.engine)
        .unwrap()
        .iter()
        .any(|skill| skill.name == NAME));
}

#[test]
fn deployment_record_path_hash_plan_and_review_tampering_are_rejected() {
    for mode in [
        "path",
        "hash",
        "json",
        "large-invalid-json",
        "plan",
        "review",
    ] {
        let f = Fixture::new();
        let p = f.install("one");
        match mode {
            "path" => {
                f.engine
                    .db
                    .conn()
                    .execute(
                        "UPDATE skills_deployments SET install_path='/outside/SKILL.md'",
                        [],
                    )
                    .unwrap();
            }
            "hash" => {
                let mut record =
                    record_for(&f.engine, &deployment_rows(&f.engine).unwrap()[0]).unwrap();
                record.content.sha256 = "0".repeat(64);
                f.engine
                    .db
                    .conn()
                    .execute(
                        "UPDATE skills_deployments SET record_json=?1",
                        [serde_json::to_string(&record).unwrap()],
                    )
                    .unwrap();
            }
            "json" => {
                f.engine
                    .db
                    .conn()
                    .execute("UPDATE skills_deployments SET record_json='{}'", [])
                    .unwrap();
            }
            "large-invalid-json" => {
                f.engine
                    .db
                    .conn()
                    .execute(
                        "UPDATE skills_deployments SET record_json=?1",
                        [" ".repeat(1024 * 1024 + 1)],
                    )
                    .unwrap();
            }
            "plan" | "review" => {
                let mut plan = f.engine.get_plan(&p.plan.id).unwrap();
                if mode == "plan" {
                    plan.entries.push(plan.entries[0].clone());
                } else {
                    plan.requires_review = false;
                }
                f.engine
                    .db
                    .conn()
                    .execute(
                        "UPDATE file_transactions SET plan_json=?1 WHERE id=?2",
                        params![serde_json::to_string(&plan).unwrap(), p.plan.id],
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            !reconcile_deployments(&f.engine).unwrap().issues.is_empty(),
            "{mode}"
        );
        assert!(!f.flag(), "{mode}");
        assert!(get_skill(&f.engine, NAME).is_err(), "{mode}");
    }
}

#[test]
fn standalone_location_replaced_by_foreign_registration_is_not_mutated() {
    let f = Fixture::new();
    let p = f.install("one");
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE skills SET source='builtin',location='/unknown/SKILL.md' WHERE name=?1",
            [NAME],
        )
        .unwrap();
    write(&p.location, markdown(NAME, "tampered"));
    let report = reconcile_deployments(&f.engine).unwrap();
    assert!(!report.issues.is_empty());
    assert!(f.flag());
    assert!(get_skill(&f.engine, NAME).is_err());
    assert!(set_enabled(&f.engine, NAME, false).is_err());
}

#[test]
fn package_skill_rejects_undeclared_backup_files() {
    let f = Fixture::new();
    install_plugin(&f);
    let location = get_skill(&f.engine, NAME).unwrap().skill.location;
    write(
        &location.with_file_name(".previous_00000000000000000000000000000000"),
        "untracked backup",
    );
    assert!(get_skill(&f.engine, NAME).is_err());
    assert!(set_enabled(&f.engine, NAME, true).is_err());
}

#[test]
fn uninstall_reinstall_and_multiple_rollback_replay_correct_state() {
    let f = Fixture::new();
    f.install("one");
    set_enabled(&f.engine, NAME, false).unwrap();
    let removed = plan_uninstall_skill(&f.engine, NAME).unwrap();
    apply(&f.engine, &removed.plan);
    reconcile_deployments(&f.engine).unwrap();
    let second = f.install("two");
    assert!(!f.flag());
    f.engine.rollback_transaction(&second.plan.id).unwrap();
    assert_eq!(reconcile_deployments(&f.engine).unwrap().removed, [NAME]);
    f.engine.rollback_transaction(&removed.plan.id).unwrap();
    let detail = get_skill(&f.engine, NAME).unwrap();
    assert!(detail.markdown.contains("Edition one"));
    assert!(!detail.skill.enabled);
    assert_eq!(f.count("skills_deployments"), 3);
}

#[test]
fn scratch_may_be_gone_after_commit_replay_uses_persisted_fingerprint() {
    let f = Fixture::new();
    let p = f.install("one");
    if let FileOp::Copy { src, .. } = &p.plan.entries[0].operation {
        fs::remove_file(src).unwrap();
    }
    assert!(get_skill(&f.reopen(), NAME).unwrap().skill.valid);
}

#[test]
fn unknown_arbitrary_directory_is_never_enumerated_or_loaded() {
    let f = Fixture::new();
    let external = f._temp.path().join("outside/SKILL.md");
    write(&external, markdown(NAME, "outside"));
    assert!(!list_skills(&f.engine)
        .unwrap()
        .iter()
        .any(|skill| skill.name == NAME));
    assert!(get_skill(&f.engine, external.to_str().unwrap()).is_err());
    insert_skill(&f, NAME, SOURCE, &external);
    let unowned = list_skills(&f.engine)
        .unwrap()
        .into_iter()
        .find(|skill| skill.name == NAME)
        .unwrap();
    assert!(!unowned.valid);
    assert!(get_skill(&f.engine, NAME).is_err());
}

#[test]
fn markdown_command_examples_are_inert_and_never_run() {
    let f = Fixture::new();
    let sentinel = f._temp.path().join("never-created");
    let text = format!(
        "{}\n```sh\ntouch '{}'\n```\n",
        markdown(NAME, "one"),
        sentinel.display()
    );
    let p = plan_install_skill(&f.engine, NAME, &text).unwrap();
    apply(&f.engine, &p.plan);
    assert_eq!(get_skill(&f.engine, NAME).unwrap().markdown, text);
    assert!(!sentinel.exists());
}

#[cfg(unix)]
#[test]
fn symlink_document_directory_staging_and_manager_root_are_rejected() {
    use std::os::unix::fs::symlink;
    for mode in ["file", "directory", "skills-root", "staging"] {
        let f = Fixture::new();
        let p = f.install("one");
        let outside = f._temp.path().join("outside");
        write(&outside.join("SKILL.md"), markdown(NAME, "one"));
        match mode {
            "file" => {
                fs::remove_file(&p.location).unwrap();
                symlink(outside.join("SKILL.md"), &p.location).unwrap();
            }
            "directory" => {
                fs::rename(
                    p.location.parent().unwrap(),
                    f.engine.paths.skills().join("parked"),
                )
                .unwrap();
                symlink(&outside, p.location.parent().unwrap()).unwrap();
            }
            "skills-root" => {
                fs::rename(
                    f.engine.paths.skills(),
                    f.engine.paths.root.join("parked-skills"),
                )
                .unwrap();
                symlink(&outside, f.engine.paths.skills()).unwrap();
            }
            "staging" => {
                fs::rename(
                    f.engine.paths.staging(),
                    f.engine.paths.root.join("parked-staging"),
                )
                .unwrap();
                symlink(&outside, f.engine.paths.staging()).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(get_skill(&f.engine, NAME).is_err(), "{mode}");
        assert!(!f.flag(), "{mode}");
        assert!(
            plan_install_skill(&f.engine, NAME, &markdown(NAME, "two")).is_err(),
            "{mode}"
        );
        assert_eq!(
            fs::read_to_string(outside.join("SKILL.md")).unwrap(),
            markdown(NAME, "one")
        );
    }
    let f = Fixture::new();
    f.install("one");
    let root = f.engine.paths.root.clone();
    fs::rename(&root, f._temp.path().join("parked-manager")).unwrap();
    fs::create_dir(f._temp.path().join("outside")).unwrap();
    symlink(f._temp.path().join("outside"), &root).unwrap();
    assert!(get_skill(&f.engine, NAME).is_err());
    assert!(plan_install_skill(&f.engine, NAME, &markdown(NAME, "two")).is_err());
}

#[cfg(unix)]
#[test]
fn executable_markdown_is_not_a_valid_skill_asset() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let p = f.install("one");
    fs::set_permissions(&p.location, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(get_skill(&f.engine, NAME).is_err());
    assert!(!f.flag());
    assert!(plan_install_skill(&f.engine, NAME, &markdown(NAME, "two")).is_err());
}

#[cfg(unix)]
#[test]
fn symlinked_trash_root_is_rejected_before_any_manifest_write() {
    let f = Fixture::new();
    f.install("one");
    let outside = f._temp.path().join("outside-trash");
    fs::create_dir(&outside).unwrap();
    fs::rename(
        f.engine.paths.trash(),
        f.engine.paths.root.join("parked-trash"),
    )
    .unwrap();
    std::os::unix::fs::symlink(&outside, f.engine.paths.trash()).unwrap();
    let count = f.count("file_transactions");
    assert!(plan_uninstall_skill(&f.engine, NAME).is_err());
    assert_eq!(f.count("file_transactions"), count);
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    assert!(location(&f.engine, NAME).unwrap().exists());
}
