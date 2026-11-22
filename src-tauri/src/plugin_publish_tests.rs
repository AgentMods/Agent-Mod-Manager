use super::*;
use crate::{db::Db, storage::AppPaths};
use serde_json::json;
use std::io::Read;

const SLUG: &str = "publish-test";
struct Fixture {
    _temp: tempfile::TempDir,
    engine: FileEngine,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("manager")).unwrap();
        let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
        migrate(&engine.db.conn()).unwrap();
        Self {
            _temp: temp,
            engine,
        }
    }
    fn draft(&self) -> plugin_workflow::DraftPreview {
        self.draft_with_origin(false)
    }
    fn draft_with_origin(&self, agent: bool) -> plugin_workflow::DraftPreview {
        let (adapter, skill) = content();
        let draft =
            plugin_workflow::plan_write_draft(&self.engine, SLUG, &adapter, &skill).unwrap();
        if agent {
            plugin_workflow::mark_agent_draft(&self.engine, &draft.draft.id).unwrap();
        }
        self.engine.approve_plan(&draft.plan.id).unwrap();
        self.engine.apply_file_ops(&draft.plan.id).unwrap();
        draft
    }
    fn preview(
        &self,
    ) -> (
        plugin_workflow::DraftPreview,
        PublicationPreview,
        PackageBytes,
    ) {
        let draft = self.draft();
        let (preview, bytes) = prepare_package(&self.engine, &draft.draft.id, metadata()).unwrap();
        (draft, preview, bytes)
    }
    fn approved(&self) -> (plugin_workflow::DraftPreview, PublicationRecord) {
        let (draft, preview, _) = self.preview();
        let record = create_publication(&self.engine, &preview).unwrap();
        let record = approve_publication(&self.engine, &record.id, &preview.digest).unwrap();
        (draft, record)
    }
    fn reopen(&self) -> FileEngine {
        FileEngine::new(
            Db::open(&self.engine.paths.db_file()).unwrap(),
            (*self.engine.paths).clone(),
        )
    }
    fn count(&self, table: &str) -> usize {
        self.engine
            .db
            .conn()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
}
fn content() -> (String, String) {
    let mut value: serde_json::Value =
        serde_json::from_str(plugins::test_fixtures::PACKAGES[0].1).unwrap();
    value["slug"] = json!(SLUG);
    value["name"] = json!("Publish Test");
    value["version"] = json!("1.2.3");
    (
        serde_json::to_string_pretty(&value).unwrap(),
        format!(
            "---\nname: {SLUG}\ndescription: Publication fixture\n---\n\nExact raw skill text.\n"
        ),
    )
}
fn metadata() -> PublicationMetadata {
    PublicationMetadata {
        title: "Publish Test".into(),
        summary: "A declarative adapter".into(),
        description: "Full description\nwith exact whitespace.\n".into(),
        tags: vec!["adapter".into()],
        game_ids: vec![7, 11],
    }
}
fn step(f: &Fixture, r: &PublicationRecord, s: PublicationStep) -> PublicationRecord {
    begin_step(&f.engine, &r.id, &r.preview.digest, s).unwrap();
    finish_step(
        &f.engine,
        &r.id,
        s,
        Some(42),
        if s == PublicationStep::PublishWork {
            Some(91)
        } else {
            None
        },
    )
    .unwrap()
}

#[test]
fn deterministic_zip_has_exact_entries_raw_text_hashes_and_no_preview_side_effects() {
    let f = Fixture::new();
    let draft = f.draft();
    let files_before = walkdir::WalkDir::new(&f.engine.paths.root)
        .into_iter()
        .count();
    let (preview, bytes) = prepare_package(&f.engine, &draft.draft.id, metadata()).unwrap();
    let (second, second_bytes) = prepare_package(&f.engine, &draft.draft.id, metadata()).unwrap();
    assert_eq!(preview, second);
    assert_eq!(bytes.as_slice(), second_bytes.as_slice());
    assert_eq!(f.count("plugin_publications"), 0);
    assert_eq!(
        files_before,
        walkdir::WalkDir::new(&f.engine.paths.root)
            .into_iter()
            .count()
    );
    assert_eq!(preview.package.destination, DESTINATION);
    assert_eq!(preview.package.archive_name, "publish-test-1.2.3.zip");
    assert_eq!(preview.package.content_type, "application/zip");
    assert_eq!(preview.package.work_status, "published");
    assert_eq!(preview.package.archive_size, bytes.len() as u64);
    assert_eq!(preview.package.archive_sha256, sha256(bytes.as_slice()));
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    assert_eq!(zip.len(), 2);
    for (i, expected) in ["adapter.json", "SKILL.md"].iter().enumerate() {
        let mut entry = zip.by_index(i).unwrap();
        assert_eq!(entry.name(), *expected);
        assert_eq!(entry.compression(), zip::CompressionMethod::Stored);
        assert_eq!(entry.unix_mode().unwrap() & 0o777, 0o644);
        let mut raw = String::new();
        entry.read_to_string(&mut raw).unwrap();
        assert_eq!(raw, preview.package.files[i].text);
        assert_eq!(sha256(raw.as_bytes()), preview.package.files[i].sha256);
        assert_eq!(raw.len() as u64, preview.package.files[i].size);
    }
    let json = serde_json::to_value(&preview).unwrap();
    assert_eq!(json["package"]["gameIds"], json!([7, 11]));
    assert!(json["package"].get("archive_sha256").is_none());
    assert!(json["package"].get("bytes").is_none());
}

#[test]
fn metadata_bounds_are_unicode_character_bounds_and_not_silently_normalized() {
    let good = metadata();
    assert!(good.validate().is_ok());
    let mut max = good.clone();
    max.title = "\u{754c}".repeat(160);
    max.summary = "x".repeat(500);
    max.description = "x".repeat(50_000);
    max.tags = (0..5).map(|n| format!("{n}{}", "x".repeat(19))).collect();
    max.game_ids = (1..=10).collect();
    assert!(max.validate().is_ok());
    let mut bad = Vec::new();
    for title in [
        "".to_string(),
        "a".into(),
        "  ".into(),
        "\na".into(),
        "x".repeat(161),
    ] {
        let mut m = good.clone();
        m.title = title;
        bad.push(m);
    }
    let mut m = good.clone();
    m.summary = "x".repeat(501);
    bad.push(m);
    let mut m = good.clone();
    m.description = "x".repeat(50_001);
    bad.push(m);
    let mut m = good.clone();
    m.description = "secret\0text".into();
    bad.push(m);
    for tags in [
        vec!["x".into(); 6],
        vec!["x".repeat(21)],
        vec!["".into()],
        vec!["a".into(), "a".into()],
        vec![" a".into()],
        vec!["a\nb".into()],
    ] {
        let mut m = good.clone();
        m.tags = tags;
        bad.push(m);
    }
    for ids in [vec![0], vec![-1], vec![1, 1], (1..=11).collect()] {
        let mut m = good.clone();
        m.game_ids = ids;
        bad.push(m);
    }
    for m in bad {
        assert!(m.validate().is_err(), "{m:?}");
    }
    let mut unknown = serde_json::to_value(good).unwrap();
    unknown["apiKey"] = json!("not accepted");
    assert!(serde_json::from_value::<PublicationMetadata>(unknown).is_err());
}

#[test]
fn committed_valid_provenance_required_and_arbitrary_ids_never_resolve_paths() {
    let f = Fixture::new();
    let (adapter, skill) = content();
    let pending = plugin_workflow::plan_write_draft(&f.engine, SLUG, &adapter, &skill).unwrap();
    assert!(prepare_package(&f.engine, &pending.draft.id, metadata()).is_err());
    f.engine.approve_plan(&pending.plan.id).unwrap();
    assert!(prepare_package(&f.engine, &pending.draft.id, metadata()).is_err());
    f.engine.apply_file_ops(&pending.plan.id).unwrap();
    assert!(prepare_package(&f.engine, &pending.draft.id, metadata()).is_ok());
    f.engine.rollback_transaction(&pending.plan.id).unwrap();
    assert!(prepare_package(&f.engine, &pending.draft.id, metadata()).is_err());
    for id in [
        "../adapter.json",
        "/tmp/private",
        "draft_fake",
        "https://attacker.example",
    ] {
        assert!(prepare_package(&f.engine, id, metadata()).is_err());
    }
    let bad = plugin_workflow::plan_write_draft(&f.engine, SLUG, "{}", &skill).unwrap();
    f.engine.approve_plan(&bad.plan.id).unwrap();
    f.engine.apply_file_ops(&bad.plan.id).unwrap();
    assert!(prepare_package(&f.engine, &bad.draft.id, metadata()).is_err());
}

#[test]
fn stale_source_blocks_creation_approval_artifact_and_begin_without_erasing_history() {
    let f = Fixture::new();
    let (draft, r) = f.approved();
    let original = fs::read(draft.draft.path.join("SKILL.md")).unwrap();
    fs::write(draft.draft.path.join("SKILL.md"), b"changed").unwrap();
    assert!(create_publication(&f.engine, &r.preview).is_err());
    assert!(approve_publication(&f.engine, &r.id, &r.preview.digest).is_err());
    assert!(verified_upload_artifact(&f.engine, &r.id, &r.preview.digest).is_err());
    assert!(begin_step(
        &f.engine,
        &r.id,
        &r.preview.digest,
        PublicationStep::CreateUpload
    )
    .is_err());
    assert_eq!(get_publication(&f.engine, &r.id).unwrap(), r);
    assert_eq!(f.count("plugin_publications"), 1);
    fs::write(draft.draft.path.join("SKILL.md"), original).unwrap();
    assert!(verified_upload_artifact(&f.engine, &r.id, &r.preview.digest).is_ok());
    fs::write(draft.draft.path.join("extra.key"), "do not leak").unwrap();
    assert!(prepare_package(&f.engine, &draft.draft.id, metadata()).is_err());
    assert_eq!(f.count("plugin_drafts"), 1);
}

#[cfg(unix)]
#[test]
fn package_and_parent_symlinks_are_rejected_even_with_matching_content() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let (draft, r) = f.approved();
    let target = draft.draft.path.join("SKILL.md");
    let outside = f._temp.path().join("outside.md");
    fs::rename(&target, &outside).unwrap();
    symlink(&outside, &target).unwrap();
    assert!(verified_upload_artifact(&f.engine, &r.id, &r.preview.digest).is_err());
    fs::remove_file(&target).unwrap();
    fs::rename(&outside, &target).unwrap();
    let directory = draft.draft.path.parent().unwrap();
    let moved = f._temp.path().join("moved");
    fs::rename(directory, &moved).unwrap();
    symlink(&moved, directory).unwrap();
    assert!(prepare_package(&f.engine, &draft.draft.id, metadata()).is_err());
}

#[test]
fn exact_consent_and_idempotent_preparation_survive_restart_metadata_change_gets_new_id() {
    let f = Fixture::new();
    let (draft, preview, bytes) = f.preview();
    let r = create_publication(&f.engine, &preview).unwrap();
    assert!(verified_upload_artifact(&f.engine, &r.id, &preview.digest).is_err());
    assert!(approve_publication(&f.engine, &r.id, "wrong digest").is_err());
    assert!(begin_step(
        &f.engine,
        &r.id,
        &preview.digest,
        PublicationStep::CreateUpload
    )
    .is_err());
    let r = approve_publication(&f.engine, &r.id, &preview.digest).unwrap();
    let events = f.count("plugin_publication_events");
    assert_eq!(
        approve_publication(&f.engine, &r.id, &preview.digest).unwrap(),
        r
    );
    assert_eq!(events, f.count("plugin_publication_events"));
    let engine = f.reopen();
    migrate(&engine.db.conn()).unwrap();
    assert_eq!(create_publication(&engine, &preview).unwrap(), r);
    assert_eq!(
        verified_upload_artifact(&engine, &r.id, &preview.digest)
            .unwrap()
            .1
            .as_slice(),
        bytes.as_slice()
    );
    let mut m = metadata();
    m.title = "New title, new consent".into();
    let (changed, _) = prepare_package(&engine, &draft.draft.id, m).unwrap();
    let new = create_publication(&engine, &changed).unwrap();
    assert_ne!(new.id, r.id);
    assert_ne!(new.preview.digest, r.preview.digest);
    assert!(new.approved_digest.is_none());
    assert_eq!(f.count("plugin_publications"), 2);
    assert_eq!(f.count("plugin_drafts"), 1);
}

#[test]
fn altered_persisted_preview_destination_metadata_raw_text_and_zip_digest_are_rejected() {
    let f = Fixture::new();
    let (_, r) = f.approved();
    let original = serde_json::to_value(&r.preview).unwrap();
    for (key, value) in [
        ("destination", json!("https://attacker.example/api")),
        ("title", json!("Altered approved title")),
        ("archiveSha256", json!("0".repeat(64))),
        ("archiveSize", json!(1)),
        ("workStatus", json!("draft")),
        ("archiveName", json!("../../private.zip")),
        ("contentType", json!("text/plain")),
    ] {
        let mut changed = original.clone();
        changed["package"][key] = value;
        f.engine
            .db
            .conn()
            .execute(
                "UPDATE plugin_publications SET preview_json=?1 WHERE id=?2",
                params![serde_json::to_string(&changed).unwrap(), r.id],
            )
            .unwrap();
        assert!(get_publication(&f.engine, &r.id).is_err(), "{key}");
    }
    let mut changed = original.clone();
    changed["package"]["files"][1]["text"] = json!("tampered");
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE plugin_publications SET preview_json=?1 WHERE id=?2",
            params![serde_json::to_string(&changed).unwrap(), r.id],
        )
        .unwrap();
    assert!(get_publication(&f.engine, &r.id).is_err());
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE plugin_publications SET preview_json=?1 WHERE id=?2",
            params![serde_json::to_string(&original).unwrap(), r.id],
        )
        .unwrap();
    assert_eq!(get_publication(&f.engine, &r.id).unwrap(), r);
    let mut forged = r.preview.clone();
    forged.package.destination = "https://attacker.example".into();
    assert!(create_publication(&f.engine, &forged).is_err());
}

#[test]
fn durable_steps_are_ordered_unknown_is_not_retried_and_ready_scan_can_be_polled() {
    let f = Fixture::new();
    let (_, r) = f.approved();
    let id = &r.id;
    let digest = &r.preview.digest;
    assert!(begin_step(&f.engine, id, digest, PublicationStep::PublishWork).is_err());
    let pending = begin_step(&f.engine, id, digest, PublicationStep::CreateUpload).unwrap();
    assert_eq!(pending.phase, PublicationPhase::CreateUploadPending);
    assert!(begin_step(&f.engine, id, digest, PublicationStep::CreateUpload).is_err());
    assert!(finish_step(&f.engine, id, PublicationStep::CreateUpload, None, None).is_err());
    assert!(finish_step(&f.engine, id, PublicationStep::CreateUpload, Some(0), None).is_err());
    let unknown = mark_unknown(&f.engine, id, PublicationStep::CreateUpload).unwrap();
    assert_eq!(unknown.state, PublicationState::Unknown);
    assert_eq!(
        mark_unknown(&f.engine, id, PublicationStep::CreateUpload).unwrap(),
        unknown
    );
    assert!(begin_step(&f.engine, id, digest, PublicationStep::CreateUpload).is_err());
    assert!(fail_step(&f.engine, id, PublicationStep::CreateUpload).is_err());
    let created =
        finish_step(&f.engine, id, PublicationStep::CreateUpload, Some(42), None).unwrap();
    assert_eq!(created.phase, PublicationPhase::UploadCreated);
    assert_eq!(
        finish_step(&f.engine, id, PublicationStep::CreateUpload, Some(42), None).unwrap(),
        created
    );
    assert!(finish_step(&f.engine, id, PublicationStep::CreateUpload, Some(43), None).is_err());
    let started = step(&f, &created, PublicationStep::StartUpload);
    let uploaded = step(&f, &started, PublicationStep::PutPart);
    begin_step(&f.engine, id, digest, PublicationStep::CompleteUpload).unwrap();
    let scanning = await_scan(&f.engine, id).unwrap();
    assert_eq!(scanning.phase, PublicationPhase::AwaitingScan);
    assert_eq!(await_scan(&f.engine, id).unwrap(), scanning);
    assert!(begin_step(&f.engine, id, digest, PublicationStep::CompleteUpload).is_err());
    assert!(begin_step(&f.engine, id, digest, PublicationStep::PublishWork).is_err());
    assert!(recover_pending_publications(&f.engine).unwrap().is_empty());
    let ready = finish_step(
        &f.engine,
        id,
        PublicationStep::CompleteUpload,
        Some(42),
        None,
    )
    .unwrap();
    assert_eq!(ready.phase, PublicationPhase::UploadReady);
    let done = step(&f, &ready, PublicationStep::PublishWork);
    assert_eq!(done.state, PublicationState::Completed);
    assert_eq!(done.phase, PublicationPhase::Completed);
    assert_eq!(done.work_id, Some(91));
    assert!(begin_step(&f.engine, id, digest, PublicationStep::PublishWork).is_err());
    assert!(mark_unknown(&f.engine, id, PublicationStep::PublishWork).is_err());
    assert_eq!(uploaded.phase, PublicationPhase::PartUploaded);
    assert_eq!(get_publication(&f.reopen(), id).unwrap(), done);
    assert_eq!(f.count("plugin_publications"), 1);
    assert!(f.count("plugin_publication_events") > 10);
}

#[test]
fn crash_recovery_never_retries_pending_publish_and_history_remains() {
    let f = Fixture::new();
    let (_, mut r) = f.approved();
    for s in [
        PublicationStep::CreateUpload,
        PublicationStep::StartUpload,
        PublicationStep::PutPart,
        PublicationStep::CompleteUpload,
    ] {
        r = step(&f, &r, s);
    }
    begin_step(
        &f.engine,
        &r.id,
        &r.preview.digest,
        PublicationStep::PublishWork,
    )
    .unwrap();
    let engine = f.reopen();
    let recovered = recover_pending_publications(&engine).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].state, PublicationState::Unknown);
    assert_eq!(recovered[0].phase, PublicationPhase::PublishWorkPending);
    assert_eq!(recovered[0].file_id, Some(42));
    assert!(recover_pending_publications(&engine).unwrap().is_empty());
    assert!(begin_step(
        &engine,
        &r.id,
        &r.preview.digest,
        PublicationStep::PublishWork
    )
    .is_err());
    assert_eq!(
        create_publication(&engine, &r.preview).unwrap().state,
        PublicationState::Unknown
    );
    assert_eq!(f.count("plugin_publications"), 1);
    assert_eq!(f.count("plugin_drafts"), 1);
    assert!(f.engine.paths.staging().join("plugin-drafts").exists());
}

#[test]
fn definitive_failure_is_terminal_and_remote_secrets_have_no_persistence_field() {
    let f = Fixture::new();
    let (_, r) = f.approved();
    begin_step(
        &f.engine,
        &r.id,
        &r.preview.digest,
        PublicationStep::CreateUpload,
    )
    .unwrap();
    let failed = fail_step(&f.engine, &r.id, PublicationStep::CreateUpload).unwrap();
    assert_eq!(failed.state, PublicationState::Error);
    assert!(begin_step(
        &f.engine,
        &r.id,
        &r.preview.digest,
        PublicationStep::CreateUpload
    )
    .is_err());
    let mut remote = serde_json::to_value(&failed).unwrap();
    remote["signedURL"] = json!("https://signed.example?key=credential-sentinel");
    assert!(serde_json::from_value::<PublicationRecord>(remote).is_err());
    let persisted: String = f
        .engine
        .db
        .conn()
        .query_row(
            "SELECT preview_json FROM plugin_publications WHERE id=?1",
            [&r.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!persisted.contains("credential-sentinel"));
    let cols: Vec<String> = f
        .engine
        .db
        .conn()
        .prepare("PRAGMA table_info(plugin_publications)")
        .unwrap()
        .query_map([], |r| r.get(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for forbidden in ["api_key", "signed_url", "response_json", "token", "key"] {
        assert!(!cols.iter().any(|c| c == forbidden));
    }
    assert_eq!(f.count("plugin_publications"), 1);
}

#[test]
fn installed_external_requires_matching_committed_plan_not_just_validated_flag() {
    let f = Fixture::new();
    let draft = f.draft();
    let deployment = plugin_workflow::plan_install_plugin(&f.engine, &draft.draft.id).unwrap();
    f.engine.approve_plan(&deployment.plan.id).unwrap();
    f.engine.apply_file_ops(&deployment.plan.id).unwrap();
    plugin_workflow::reconcile_deployments(&f.engine).unwrap();
    let (preview, _) = prepare_installed_package(&f.engine, SLUG, metadata()).unwrap();
    assert!(preview.package.draft_id.is_none());
    assert!(matches!(
        preview.source,
        PublicationSource::Installed {
            deployment_transaction_id: Some(_),
            ..
        }
    ));
    f.engine
        .db
        .conn()
        .execute("UPDATE plugins SET validated=0 WHERE slug=?1", [SLUG])
        .unwrap();
    assert!(prepare_installed_package(&f.engine, SLUG, metadata()).is_err());
    f.engine
        .db
        .conn()
        .execute("UPDATE plugins SET validated=1 WHERE slug=?1", [SLUG])
        .unwrap();
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE file_transactions SET status='rolled_back' WHERE id=?1",
            [&deployment.plan.id],
        )
        .unwrap();
    assert!(prepare_installed_package(&f.engine, SLUG, metadata()).is_err());
    assert!(prepare_installed_package(&f.engine, "../private", metadata()).is_err());
}

#[test]
fn former_adapter_slug_publishes_only_the_reviewed_external_package() {
    let f = Fixture::new();
    let (slug, adapter, skill) = plugins::test_fixtures::PACKAGES[0];
    let reviewed_skill = format!("{skill}\nExternal package revision.\n");
    let draft =
        plugin_workflow::plan_write_draft(&f.engine, slug, adapter, &reviewed_skill).unwrap();
    f.engine.approve_plan(&draft.plan.id).unwrap();
    f.engine.apply_file_ops(&draft.plan.id).unwrap();
    let deployment = plugin_workflow::plan_install_plugin(&f.engine, &draft.draft.id).unwrap();
    f.engine.approve_plan(&deployment.plan.id).unwrap();
    f.engine.apply_file_ops(&deployment.plan.id).unwrap();
    plugin_workflow::reconcile_deployments(&f.engine).unwrap();

    let (preview, _) = prepare_installed_package(&f.engine, slug, metadata()).unwrap();
    assert_eq!(
        preview.source,
        PublicationSource::Installed {
            slug: slug.into(),
            source: "external".into(),
            deployment_transaction_id: Some(deployment.plan.id),
        }
    );
    assert_eq!(preview.package.files[1].text, reviewed_skill);
    let publication = create_publication(&f.engine, &preview).unwrap();
    approve_publication(&f.engine, &publication.id, &preview.digest).unwrap();
    fs::write(deployment.install_path.join("SKILL.md"), skill).unwrap();
    assert!(prepare_installed_package(&f.engine, slug, metadata()).is_err());
    assert!(verified_upload_artifact(&f.engine, &publication.id, &preview.digest).is_err());
}

#[test]
fn history_listing_pages_through_all_records() {
    let f = Fixture::new();
    let (_, preview, _) = f.preview();
    // Fixtures use the real preview construction but one transaction avoids
    // making this pagination test perform 105 independent provenance reads.
    let mut conn = f.engine.db.conn();
    let tx = conn.transaction().unwrap();
    for n in 0..105 {
        let mut m = metadata();
        m.title = format!("Publication {n}");
        let (p, _) = build_preview(
            preview.source.clone(),
            SLUG,
            preview.package.files.clone(),
            m,
        )
        .unwrap();
        tx.execute("INSERT INTO plugin_publications(id,preview_json,digest,state,phase,created_at,updated_at) VALUES (?1,?2,?3,'intended','prepared','now','now')",params![util::new_id("publish"),serde_json::to_string(&p).unwrap(),p.digest]).unwrap();
    }
    tx.commit().unwrap();
    drop(conn);
    let first = list_publications(&f.engine, None).unwrap();
    assert_eq!(first.len(), 100);
    let next = list_publications(&f.engine, Some(&first.last().unwrap().id)).unwrap();
    assert_eq!(next.len(), 5);
    assert!(next.iter().all(|p| first.iter().all(|row| row.id != p.id)));
    assert!(list_publications(&f.engine, Some(&next.last().unwrap().id))
        .unwrap()
        .is_empty());
    assert_eq!(f.count("plugin_publications"), 105);
}

#[test]
fn archive_builder_rejects_paths_duplicates_sizes_and_hashes() {
    let f = Fixture::new();
    let (_, preview, _) = f.preview();
    for name in [
        "../adapter.json",
        "/adapter.json",
        "C:/adapter.json",
        "SKILL.md",
    ] {
        let mut files = preview.package.files.clone();
        files[0].name = name.into();
        assert!(archive(&files).is_err());
    }
    let mut files = preview.package.files.clone();
    files[0].size += 1;
    assert!(archive(&files).is_err());
    let mut files = preview.package.files.clone();
    files[0].sha256 = "0".repeat(64);
    assert!(archive(&files).is_err());
}

#[test]
fn large_publication_retains_exact_zip_and_approval_across_reopen() {
    let f = Fixture::new();
    let (adapter, skill) = content();
    let mut adapter: serde_json::Value = serde_json::from_str(&adapter).unwrap();
    adapter["uninstallNotes"] = json!(["注销说明".repeat(32 * 1024)]);
    let adapter = adapter.to_string();
    let skill = format!("{skill}{}", "发布说明\n".repeat(96 * 1024));
    let draft = plugin_workflow::plan_write_draft(&f.engine, SLUG, &adapter, &skill).unwrap();
    f.engine.approve_plan(&draft.plan.id).unwrap();
    f.engine.apply_file_ops(&draft.plan.id).unwrap();
    let (preview, bytes) = prepare_package(&f.engine, &draft.draft.id, metadata()).unwrap();
    assert!(bytes.len() > 1024 * 1024);
    let record = create_publication(&f.engine, &preview).unwrap();
    assert!(verified_upload_artifact(&f.engine, &record.id, &preview.digest).is_err());
    approve_publication(&f.engine, &record.id, &preview.digest).unwrap();
    let engine = f.reopen();
    let (saved, rebuilt) = verified_upload_artifact(&engine, &record.id, &preview.digest).unwrap();
    assert_eq!(saved.preview, preview);
    assert_eq!(rebuilt.as_slice(), bytes.as_slice());
    let install = plugin_workflow::plan_install_plugin(&engine, &draft.draft.id).unwrap();
    engine.approve_plan(&install.plan.id).unwrap();
    engine.apply_file_ops(&install.plan.id).unwrap();
    plugin_workflow::reconcile_deployments(&engine).unwrap();
    let (installed, installed_bytes) =
        prepare_installed_package(&engine, SLUG, metadata()).unwrap();
    assert!(matches!(
        installed.source,
        PublicationSource::Installed { .. }
    ));
    assert_eq!(installed_bytes.as_slice(), bytes.as_slice());
    let mut zip = zip::ZipArchive::new(Cursor::new(rebuilt.as_slice())).unwrap();
    assert_eq!(zip.len(), 2);
    for (name, expected) in [("adapter.json", adapter), ("SKILL.md", skill)] {
        let mut actual = String::new();
        std::io::Read::read_to_string(&mut zip.by_name(name).unwrap(), &mut actual).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn independent_connections_cannot_claim_the_same_network_step_twice() {
    let f = Fixture::new();
    let (_, record) = f.approved();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let engine = f.reopen();
            let record = record.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                begin_step(
                    &engine,
                    &record.id,
                    &record.preview.digest,
                    PublicationStep::CreateUpload,
                )
                .is_ok()
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .filter_map(|t| t.join().ok())
            .filter(|ok| *ok)
            .count(),
        1
    );
    assert_eq!(
        get_publication(&f.engine, &record.id).unwrap().state,
        PublicationState::Pending
    );
    assert_eq!(f.count("plugin_publication_events"), 3);
}

#[test]
fn inconsistent_persisted_state_is_not_reported_as_success() {
    let f = Fixture::new();
    let (_, r) = f.approved();
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE plugin_publications SET state='completed',phase='completed' WHERE id=?1",
            [&r.id],
        )
        .unwrap();
    assert!(get_publication(&f.engine, &r.id).is_err());
}

#[test]
fn ordinary_work_consumes_agent_draft_only_after_completed_send_or_reconciliation() {
    for reconcile in [false, true] {
        let f = Fixture::new();
        let draft = f.draft_with_origin(true);
        let (preview, _) = prepare_package(&f.engine, &draft.draft.id, metadata()).unwrap();
        let record = create_publication(&f.engine, &preview).unwrap();
        let mut record = approve_publication(&f.engine, &record.id, &preview.digest).unwrap();
        for action in [
            PublicationStep::CreateUpload,
            PublicationStep::StartUpload,
            PublicationStep::PutPart,
            PublicationStep::CompleteUpload,
        ] {
            record = step(&f, &record, action);
            assert!(draft.draft.path.is_dir());
        }
        let action = PublicationStep::PublishWork;
        let pending = begin_step(&f.engine, &record.id, &preview.digest, action).unwrap();
        assert_eq!(pending.state, PublicationState::Pending);
        assert!(draft.draft.path.is_dir());

        let completed = if reconcile {
            let unknown = mark_unknown(&f.engine, &pending.id, action).unwrap();
            assert_eq!(unknown.state, PublicationState::Unknown);
            assert!(draft.draft.path.is_dir());
            finish_reconciliation(&f.engine, &unknown, Some(91)).unwrap()
        } else {
            finish_step(&f.engine, &pending.id, action, pending.file_id, Some(91)).unwrap()
        };
        assert_eq!(completed.state, PublicationState::Completed);
        assert_eq!(completed.work_id, Some(91));
        assert!(!draft.draft.path.exists());

        let reopened = f.reopen();
        assert!(plugin_workflow::list_drafts(&reopened).unwrap().is_empty());
        assert_eq!(
            get_publication(&reopened, &completed.id).unwrap(),
            completed
        );
        assert_eq!(
            finish_step(
                &reopened,
                &completed.id,
                action,
                completed.file_id,
                Some(91)
            )
            .unwrap(),
            completed
        );
        assert_eq!(list_publications(&reopened, None).unwrap(), vec![completed]);
    }
}

#[test]
fn deleted_draft_blocks_new_steps_but_preserves_completed_history_and_installed_publication() {
    let f = Fixture::new();
    let engine = &f.engine;
    let (draft, mut completed) = f.approved();
    for action in [
        PublicationStep::CreateUpload,
        PublicationStep::StartUpload,
        PublicationStep::PutPart,
        PublicationStep::CompleteUpload,
        PublicationStep::PublishWork,
    ] {
        completed = step(&f, &completed, action);
    }
    let mut changed = metadata();
    changed.title.push_str(" updated");
    let (preview, _) = prepare_package(engine, &draft.draft.id, changed).unwrap();
    let open = create_publication(engine, &preview).unwrap();
    approve_publication(engine, &open.id, &preview.digest).unwrap();
    let pending = begin_step(
        engine,
        &open.id,
        &preview.digest,
        PublicationStep::CreateUpload,
    )
    .unwrap();
    let install = plugin_workflow::plan_install_plugin(engine, &draft.draft.id).unwrap();
    engine.approve_plan(&install.plan.id).unwrap();
    engine.apply_file_ops(&install.plan.id).unwrap();
    plugin_workflow::reconcile_deployments(engine).unwrap();
    let (from_install, _) = prepare_installed_package(engine, SLUG, metadata()).unwrap();

    plugin_workflow::delete_confirmed_draft(engine, &draft.draft.id).unwrap();
    let reopened = f.reopen();
    assert_eq!(list_publications(&reopened, None).unwrap().len(), 2);
    assert_eq!(
        get_publication(&reopened, &completed.id).unwrap(),
        completed
    );
    assert_eq!(get_publication(&reopened, &pending.id).unwrap(), pending);
    assert_eq!(
        prepare_installed_package(&reopened, SLUG, metadata())
            .unwrap()
            .0,
        from_install
    );
    assert!(create_publication(&reopened, &preview).is_err());
    assert!(approve_publication(&reopened, &open.id, &preview.digest).is_err());

    // A request already authorized and durably pending is not canceled by
    // local deletion. Its outcome can still be recorded, but no new side
    // effect from that draft is authorized until it is restored.
    let observed = finish_step(
        &reopened,
        &open.id,
        PublicationStep::CreateUpload,
        Some(43),
        None,
    )
    .unwrap();
    assert_eq!(observed.phase, PublicationPhase::UploadCreated);
    assert!(matches!(
        begin_step(
            &reopened,
            &open.id,
            &preview.digest,
            PublicationStep::StartUpload
        ),
        Err(AppError::Conflict(_))
    ));
    assert_eq!(get_publication(&reopened, &open.id).unwrap(), observed);

    let item = crate::trash::list_trash(&reopened.db, false)
        .unwrap()
        .remove(0);
    let restore = crate::trash::plan_restore(&reopened, &[item.id], false).unwrap();
    reopened.approve_plan(&restore.id).unwrap();
    reopened.apply_file_ops(&restore.id).unwrap();
    assert_eq!(
        begin_step(
            &reopened,
            &open.id,
            &preview.digest,
            PublicationStep::StartUpload
        )
        .unwrap()
        .phase,
        PublicationPhase::StartUploadPending
    );
}
