use super::{tests::setup, *};
use std::io::Write;

fn zip(path: &Path, files: &[(&str, &[u8])], directories: &[&str]) {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    for directory in directories {
        zip.add_directory(*directory, options).unwrap();
    }
    for (name, data) in files {
        zip.start_file(*name, options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

fn plan(agent: &FileEngine, ops: Vec<FileOp>) -> FilePlan {
    agent.plan_file_ops(ops, "归档测试".into()).unwrap()
}

fn compress(src: &Path, dst: &Path) -> FileOp {
    FileOp::Compress {
        src: src.into(),
        dst: dst.into(),
    }
}

fn extract(src: &Path, dst: &Path) -> FileOp {
    FileOp::Extract {
        src: src.into(),
        dst: dst.into(),
    }
}

#[test]
fn archive_ops_roundtrip_and_rollback_keep_sources_and_original_wrapper() {
    let (dir, engine, _) = setup();
    let agent = engine.for_local_agent();
    let source = dir.path().join("原始文件夹");
    fs::create_dir_all(source.join("empty")).unwrap();
    fs::create_dir(source.join("nested")).unwrap();
    fs::write(source.join("nested/message.txt"), "hello").unwrap();
    for extension in ["ZIP", "7z"] {
        let archive = dir.path().join(format!("new.{extension}"));
        let packing = plan(&agent, vec![compress(&source, &archive)]);
        assert_eq!(packing.entries[0].before, None);
        assert_eq!(packing.entries[0].source, fingerprint(&source).unwrap());
        assert_eq!(packing.total_bytes, 5);
        assert!(packing.requires_review);
        assert!(!archive.exists());
        assert!(engine.get_plan(&packing.id).is_err());
        assert!(agent.apply_file_ops(&packing.id).is_err());
        agent.approve_plan(&packing.id).unwrap();
        assert!(engine.apply_file_ops(&packing.id).is_err());
        agent.apply_file_ops(&packing.id).unwrap();
        let destination = dir.path().join(format!("new-parent-{extension}/unpacked"));
        let unpacking = plan(&agent, vec![extract(&archive, &destination)]);
        assert_eq!(unpacking.entries[0].source, fingerprint(&archive).unwrap());
        assert!(!destination.parent().unwrap().exists());
        agent.approve_plan(&unpacking.id).unwrap();
        agent.apply_file_ops(&unpacking.id).unwrap();
        let unpacked = destination.join("原始文件夹");
        assert_eq!(
            fingerprint(&unpacked).unwrap(),
            fingerprint(&source).unwrap()
        );
        let (_, journal) = engine
            .db
            .conn()
            .query_row(
                "SELECT status, journal_json FROM file_transactions WHERE id=?1",
                [&unpacking.id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .unwrap();
        let journal: Vec<JournalEntry> = serde_json::from_str(&journal).unwrap();
        let rename = journal[0].renames.as_ref().unwrap().last().unwrap();
        assert_eq!(rename.expected_content, fingerprint(&destination).unwrap());
        assert!(
            !rename.from.parent().unwrap().exists(),
            "private staging should be cleaned"
        );
        assert!(engine.rollback_transaction(&unpacking.id).is_err());
        agent.rollback_transaction(&unpacking.id).unwrap();
        assert!(!destination.parent().unwrap().exists());
        assert!(archive.is_file());
        agent.rollback_transaction(&packing.id).unwrap();
        assert!(!archive.exists());
        assert_eq!(
            fs::read_to_string(source.join("nested/message.txt")).unwrap(),
            "hello"
        );
    }
}

#[test]
fn archive_ops_reject_bad_types_formats_protected_paths_and_plan_overlaps() {
    let (dir, engine, game) = setup();
    let agent = engine.for_local_agent();
    let root = dir.path();
    let source = game.join("source");
    let file = source.join("normal.txt");
    fs::create_dir(&source).unwrap();
    fs::write(&file, b"alpha").unwrap();
    let existing_zip = game.join("existing.zip");
    zip(&existing_zip, &[("normal.txt", b"alpha")], &[]);
    let output = game.join("new.zip");
    for op in [
        compress(&file, &output),
        compress(&source, &game.join("new.tar")),
        compress(&source, &existing_zip),
        compress(&source, &source.join("inside.zip")),
        compress(&source, &game.join("PRIVATE/created.zip")),
        extract(&source, &game.join("unpacked")),
        extract(&file, &game.join("unpacked")),
        extract(&existing_zip, &source),
        extract(&existing_zip, &game.join(".env.local/new")),
        extract(&existing_zip, &game.join(".amm-retained/new")),
    ] {
        assert!(
            agent
                .plan_file_ops(vec![op.clone()], "invalid".into())
                .is_err(),
            "{op:?}"
        );
    }
    for name in [".EnV.LOcaL", "Private", ".AMM-ReTAINED"] {
        let child = source.join(name);
        fs::create_dir(&child).unwrap();
        assert!(agent
            .plan_file_ops(vec![compress(&source, &output)], "protected".into())
            .is_err());
        fs::remove_dir(child).unwrap();
    }
    for other in [
        FileOp::Move {
            src: source.clone(),
            dst: root.join("park"),
        },
        FileOp::WriteText {
            dst: file.clone(),
            content: "new".into(),
        },
    ] {
        assert!(agent
            .plan_file_ops(vec![other, compress(&source, &output)], "overlap".into())
            .is_err());
    }
    assert!(engine
        .plan_file_ops(vec![compress(&source, &output)], "scoped".into())
        .is_err());
    assert!(agent
        .plan_internal(
            vec![extract(&existing_zip, &game.join("unpacked"))],
            "internal".into()
        )
        .is_err());
    assert_eq!(fs::read_to_string(file).unwrap(), "alpha");
    assert!(!output.exists());
}

#[test]
fn archive_ops_fail_closed_on_protected_extracted_members_and_leave_no_output() {
    let (dir, engine, game) = setup();
    let agent = engine.for_local_agent();
    for (member, is_dir) in [
        ("top/.ENV", false),
        ("top/.env.local", false),
        ("top\\Private\\secret.txt", false),
        ("top/.amm-retained/", true),
        ("top/privATE/", true),
        ("top/.env. /", true),
        ("top/.env:alternate", false),
        ("top/../outside.txt", false),
    ] {
        let archive = dir.path().join("untrusted.zip");
        if is_dir {
            zip(&archive, &[], &[member]);
        } else {
            zip(&archive, &[(member, b"untrusted")], &[]);
        }
        let dst = game.join("new-parent/unpacked");
        let first = game.join("before.txt");
        let p = plan(
            &agent,
            vec![
                FileOp::WriteText {
                    dst: first.clone(),
                    content: "earlier entry".into(),
                },
                extract(&archive, &dst),
            ],
        );
        agent.approve_plan(&p.id).unwrap();
        assert!(agent.apply_file_ops(&p.id).is_err(), "{member}");
        assert!(!dst.parent().unwrap().exists(), "{member}");
        assert!(!game.join("outside.txt").exists());
        assert!(!first.exists());
        assert!(archive.is_file());
        assert!(agent.recover_interrupted().unwrap().is_empty());
    }
    let archive = dir.path().join("partial-entry.zip");
    zip(&archive, &[("top/unfinished.part", b"untrusted")], &[]);
    let dst = engine.paths.staging().join("out");
    let p = plan(&agent, vec![extract(&archive, &dst)]);
    agent.approve_plan(&p.id).unwrap();
    assert!(agent.apply_file_ops(&p.id).is_err());
    assert!(!dst.exists());
}

#[test]
fn archive_ops_reject_hard_link_aliases_to_protected_files() {
    let (dir, engine, game) = setup();
    let agent = engine.for_local_agent();
    let secrets = dir.path().join(".env");
    zip(&secrets, &[("secret.txt", b"secret")], &[]);
    let tree = game.join("source");
    fs::create_dir(&tree).unwrap();
    let alias = tree.join("normal.zip");
    fs::hard_link(secrets, &alias).unwrap();
    for op in [
        compress(&tree, &game.join("out.zip")),
        extract(&alias, &game.join("unpacked")),
    ] {
        assert!(agent.plan_file_ops(vec![op], "protected".into()).is_err());
    }
}

#[cfg(unix)]
#[test]
fn archive_ops_refuse_nested_links_literal_backslashes_and_case_collisions() {
    use std::os::unix::fs::symlink;
    let (dir, engine, game) = setup();
    let agent = engine.for_local_agent();
    let secret = dir.path().join(".env");
    fs::write(&secret, "secret").unwrap();
    let tree = game.join("source");
    fs::create_dir(&tree).unwrap();
    let output = game.join("out.zip");
    let link = tree.join("ordinary");
    symlink(secret, &link).unwrap();
    assert!(agent
        .plan_file_ops(vec![compress(&tree, &output)], "link".into())
        .is_err());
    fs::remove_file(link).unwrap();
    let literal = tree.join(r"ordinary\.env");
    fs::write(&literal, "fixture").unwrap();
    assert!(agent
        .plan_file_ops(vec![compress(&tree, &output)], "ambiguous".into())
        .is_err());
    fs::remove_file(literal).unwrap();
    fs::create_dir(tree.join("left")).unwrap();
    // macOS may use a case-insensitive filesystem that forbids creating the fixture.
    if fs::create_dir(tree.join("Left")).is_ok() {
        assert!(agent
            .plan_file_ops(vec![compress(&tree, &output)], "collision".into())
            .is_err());
    }
}

#[test]
fn archive_ops_detect_source_and_committed_result_changes_without_moving_user_files() {
    let (dir, engine, game) = setup();
    let agent = engine.for_local_agent();
    let tree = game.join("tree");
    fs::create_dir(&tree).unwrap();
    let file = tree.join("data.txt");
    fs::write(&file, b"old").unwrap();
    let output = game.join("out.zip");
    let packing = plan(&agent, vec![compress(&tree, &output)]);
    agent.approve_plan(&packing.id).unwrap();
    fs::write(&file, b"new").unwrap();
    assert!(agent.apply_file_ops(&packing.id).is_err());
    assert!(!output.exists());

    let archive = dir.path().join("source.zip");
    zip(&archive, &[("data.txt", b"old")], &[]);
    let unpacked = game.join("unpacked");
    let unpacking = plan(&agent, vec![extract(&archive, &unpacked)]);
    agent.approve_plan(&unpacking.id).unwrap();
    zip(&archive, &[("data.txt", b"new")], &[]);
    assert!(agent.apply_file_ops(&unpacking.id).is_err());
    assert!(!unpacked.exists());

    let unpacking = plan(&agent, vec![extract(&archive, &unpacked)]);
    agent.approve_plan(&unpacking.id).unwrap();
    agent.apply_file_ops(&unpacking.id).unwrap();
    let data = unpacked.join("data.txt");
    fs::write(&data, b"user edit").unwrap();
    assert!(agent.rollback_transaction(&unpacking.id).is_err());
    assert_eq!(fs::read(&data).unwrap(), b"user edit");
    fs::write(&data, b"new").unwrap();
    let added = unpacked.join("Private");
    fs::create_dir(&added).unwrap();
    assert!(agent.rollback_transaction(&unpacking.id).is_err());
    assert!(added.exists());
    fs::remove_dir(&added).unwrap();
    agent.rollback_transaction(&unpacking.id).unwrap();
    assert!(!unpacked.exists());
    assert!(archive.is_file());
}

#[test]
fn archive_ops_do_not_overwrite_a_concurrent_destination_or_break_recovery() {
    let (dir, engine, game) = setup();
    let agent = engine.for_local_agent();
    let archive = dir.path().join("fixture.zip");
    zip(&archive, &[("file.txt", b"payload")], &[]);
    let dst = game.join("out");
    let p = plan(&agent, vec![extract(&archive, &dst)]);
    agent.approve_plan(&p.id).unwrap();
    let destination = paths::canonicalize_lenient(&dst).unwrap();
    *engine.before_rename.lock().unwrap() = Some(Box::new(move |step| {
        if step.to == destination {
            fs::create_dir(&destination).unwrap();
            fs::write(destination.join("existing.txt"), "external").unwrap();
        }
    }));
    assert!(agent.apply_file_ops(&p.id).is_err());
    assert_eq!(
        fs::read_to_string(dst.join("existing.txt")).unwrap(),
        "external"
    );
    assert!(!dst.join("file.txt").exists());
    assert!(agent.recover_interrupted().unwrap().is_empty());
    let status: String = engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM file_transactions WHERE id=?1",
            [&p.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "rolled_back");
}
