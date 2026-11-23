use super::*;
use crate::{agent::policy::PermissionMode, mcp};
use std::{fs, io::Write};

const FILENAME: &str = "Release build.zip";

fn setup_with_stock_playwright() -> (tempfile::TempDir, FileEngine, PathBuf) {
    let (dir, engine, game) = crate::transaction::tests::setup();
    mcp::McpServers::new(engine.db.clone())
        .seed_presets()
        .unwrap();
    (dir, engine, game)
}

fn setup() -> (tempfile::TempDir, FileEngine, PathBuf) {
    let (dir, engine, game) = setup_with_stock_playwright();
    // The receipt tools must not use Playwright when its launcher was customized.
    let servers = mcp::McpServers::new(engine.db.clone());
    let mut playwright = servers.get("playwright").unwrap();
    playwright.args.push("--fixture-disabled".into());
    servers.update(&playwright).unwrap();
    (dir, engine, game)
}

fn incoming(engine: &FileEngine) -> PathBuf {
    let path = engine.paths.downloads().join("incoming");
    assert!(path.is_dir(), "storage provisions downloads/incoming");
    path
}

fn zip_at(path: &Path, bytes: &[u8]) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    zip.start_file("notes.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(bytes).unwrap();
    zip.finish().unwrap();
}

fn context(engine: FileEngine, resolution_scope: ResolutionScope) -> ToolContext {
    let resolution_scope = match resolution_scope {
        ResolutionScope::Agent(_) => ResolutionScope::Agent(
            crate::history::History::new(engine.db.clone())
                .create_conversation("Fixture", None)
                .unwrap()
                .id,
        ),
        other => other,
    };
    ToolContext {
        engine,
        resolution_scope,
        cancel: CancellationToken::new(),
        progress: None,
    }
}

#[test]
fn requires_a_portable_archive_basename_and_scrubs_rejected_history() {
    let valid = json!({"gameName":" Game ","filename":FILENAME,"modName":"Release","projectPage":"https://example.com/projects/release"});
    let parsed = Args::parse(valid.clone()).unwrap();
    assert_eq!(parsed.game_name.as_deref(), Some("Game"));
    assert_eq!(parsed.name(), "Release");
    let long_name = "公开游戏名".repeat(200);
    let args = json!({"gameName":long_name,"filename":FILENAME});
    assert_eq!(
        Args::parse(args).unwrap().game_name.as_deref(),
        Some(long_name.as_str())
    );
    assert!(
        ReceivedModTool.definition().parameters["properties"]["gameName"]
            .get("maxLength")
            .is_none()
    );
    assert!(history_arguments(&valid.to_string()).contains("projects/release"));
    for rejected in [
        json!({"gameId":"g","gameName":"Game","filename":FILENAME}),
        json!({"gameName":format!("{long_name}\u{202e}bad"),"filename":FILENAME}),
        json!({"gameId":"g","modName":"x".repeat(MAX_NAME_BYTES + 1),"filename":FILENAME}),
        json!({"filename":FILENAME}),
        json!({"gameId":"g","filename":FILENAME,"path":"private/archive.zip"}),
        json!({"gameId":"g","filename":FILENAME,"projectPage":"https://example.com/a?sig=secret"}),
        json!({"gameId":"g","filename":FILENAME,"projectPage":"https://me:secret@example.com/a"}),
        json!({"gameId":"g","filename":FILENAME,"projectPage":"file:///secret"}),
        json!({"gameId":"g","filename":FILENAME,"projectPage":"https://127.0.0.1/secret"}),
    ] {
        assert!(Args::parse(rejected.clone()).is_err(), "{rejected}");
        assert_eq!(
            ReceivedModTool
                .history_arguments(&rejected.to_string())
                .as_deref(),
            Some("{}")
        );
    }
    assert_eq!(
        ReceiptFoldersTool.history_arguments(" {} ").as_deref(),
        Some("{}")
    );
    assert_eq!(
        ReceiptFoldersTool
            .history_arguments(r#"{"path":"private/secret.zip"}"#)
            .as_deref(),
        Some("null")
    );
    for rejected in [
        "C:\\Downloads\\archive.zip",
        "../archive.zip",
        "sub/archive.zip",
        "archive.zip:stream",
        "foo\\archive.zip",
        ".env.zip",
        "CON.zip",
        "LPT¹.zip",
        "archive.zip ",
        "archive.exe",
        "archive.zip.crdownload",
        "archive.rar.crdownload",
        "bad\u{202e}.zip",
        " archive.zip",
    ] {
        assert!(archive_kind(rejected).is_err(), "{rejected}");
    }
    for filename in [
        FILENAME,
        "mod.JAR",
        "test.mrpack",
        "mod.tar.gz",
        "mod.tgz",
        "mod-tar-1.gz",
        "中文.7z",
        "mod.RaR",
    ] {
        assert!(archive_kind(filename).is_ok(), "{filename}");
    }
}

#[test]
fn chooses_only_actual_files_in_the_fixed_folders_and_rejects_ambiguity() {
    let (dir, engine, _) = setup_with_stock_playwright();
    let incoming = incoming(&engine);
    let downloads = engine.paths.downloads();
    let playwright = downloads.join("Playwright");
    let nested = playwright.join("nested");
    let sibling = downloads.join("Playwright-other");
    fs::create_dir(&nested).unwrap();
    fs::create_dir(&sibling).unwrap();
    let folders = Folders::configured(&engine).unwrap();
    for other in [downloads.as_path(), dir.path(), &nested, &sibling] {
        let path = other.join(FILENAME);
        zip_at(&path, b"not directly in an inbox");
        let bytes = fs::read(&path).unwrap();
        let receipt = ApprovedArtifact {
            artifact_id: "received-denied",
            path: &path,
            filename: FILENAME,
            size: bytes.len() as u64,
            sha256: &hex::encode(Sha256::digest(&bytes)),
        };
        assert!(matches!(
            mod_library::ingest_received(&engine, &receipt, "Game", Some("g"), FILENAME, None),
            Err(AppError::Denied(_))
        ));
    }
    assert!(matches!(folders.find(FILENAME), Err(AppError::NotFound(_))));

    zip_at(&playwright.join(FILENAME), b"browser");
    let actual = folders.find(FILENAME).unwrap();
    assert!(matches!(actual.kind, Kind::Playwright));
    assert_eq!(actual.path, playwright.join(FILENAME));
    zip_at(&incoming.join(FILENAME), b"generic MCP");
    assert!(matches!(folders.find(FILENAME), Err(AppError::Conflict(_))));

    let other = incoming.join("directory.zip");
    fs::create_dir(&other).unwrap();
    assert!(folders.find("directory.zip").is_err());
}

#[test]
fn rejects_hard_links_and_symlinked_receipts_or_roots() {
    let (_dir, engine, _) = setup_with_stock_playwright();
    let folders = Folders::configured(&engine).unwrap();
    let playwright = folders.playwright.as_ref().unwrap();
    let private = engine.paths.root.join("private");
    fs::create_dir(&private).unwrap();
    let source = private.join("secret.zip");
    zip_at(&source, b"private");
    for inbox in [&folders.incoming, playwright] {
        let link = inbox.join(FILENAME);
        fs::hard_link(&source, &link).unwrap();
        assert!(folders.find(FILENAME).is_err());
        fs::remove_file(link).unwrap();
    }

    fs::remove_dir(playwright).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&private, playwright).unwrap();
    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(&private, playwright).is_err() {
        return; // Windows may not grant Developer Mode / symlink privileges.
    }
    assert!(Folders::configured(&engine).unwrap().playwright.is_none());
    assert!(folders.find("secret.zip").is_err());
}

#[tokio::test]
async fn receipt_tools_only_accept_desktop_agent_scope() {
    let (_dir, engine, _) = setup();
    let received_args = json!({"gameId":"g","filename":FILENAME});
    for scope in [
        ResolutionScope::Desktop,
        ResolutionScope::Mcp("fixture".into()),
    ] {
        let context = context(engine.clone(), scope);
        assert!(matches!(
            ReceiptFoldersTool.prepare(&context, &json!({})).await,
            Err(AppError::Denied(_))
        ));
        assert!(matches!(
            ReceivedModTool.prepare(&context, &received_args).await,
            Err(AppError::Denied(_))
        ));
    }
    assert_eq!(
        ReceivedModTool.definition().effect,
        Effect::UncatalogedDownload
    );
}

#[tokio::test]
async fn full_trust_files_a_received_mod_without_requesting_approval() {
    use crate::agent::mod_library_flow_tests::{
        final_reply, model_tool_call, next_event, scripted_agent, turn_request,
    };
    let (_dir, engine, _) = setup();
    zip_at(&incoming(&engine).join(FILENAME), b"completed download");
    let (agent, conv, mut events, model) = scripted_agent(engine, Some("g"), 2, |step, _| {
        if step == 0 {
            model_tool_call(NAME, "receipt", json!({"gameId":"g","filename":FILENAME}))
        } else {
            final_reply()
        }
    })
    .await;
    agent
        .start_turn(turn_request(
            &conv,
            "归档刚下载的 Mod",
            PermissionMode::FullTrust,
        ))
        .unwrap();
    let result = next_event(&mut events, "tool-result").await;
    assert_eq!(result.payload["name"], NAME);
    assert_eq!(result.payload["status"], "done", "{}", result.payload);
    assert_eq!(
        next_event(&mut events, "done").await.payload["status"],
        "done"
    );
    let entries = mod_library::list_for_game(&agent.engine.db, "g").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(result.payload["result"]["artifactId"], entries[0].id);
    assert!(!agent
        .events
        .replay(&conv, 0, 100)
        .unwrap()
        .iter()
        .any(|e| e.kind == "approval-request"));
    model.await.unwrap();
}

#[tokio::test]
async fn rechecks_approved_identity_bytes_and_metadata() {
    let (_dir, engine, _) = setup();
    let incoming = incoming(&engine);
    let path = incoming.join(FILENAME);
    zip_at(&path, b"approved payload");
    let context = context(engine, ResolutionScope::Agent("fixture".into()));
    let args = json!({"gameName":"Game","filename":FILENAME});
    let prepared = ReceivedModTool.prepare(&context, &args).await.unwrap();
    assert_eq!(prepared.preview["gameId"], "g");
    assert_eq!(prepared.preview["receiptFolder"], "incoming");
    let mut unexpected = args.clone();
    unexpected["modName"] = json!("Unapproved display name");
    assert!(matches!(
        ReceivedModTool
            .execute(&context, unexpected, prepared)
            .await,
        Err(AppError::Conflict(_))
    ));

    let prepared = ReceivedModTool.prepare(&context, &args).await.unwrap();
    zip_at(&path, b"different bytes");
    assert!(matches!(
        ReceivedModTool
            .execute(&context, args.clone(), prepared)
            .await,
        Err(AppError::Conflict(_))
    ));

    let prepared = ReceivedModTool.prepare(&context, &args).await.unwrap();
    let old = incoming.join("old.zip");
    fs::rename(&path, &old).unwrap();
    fs::copy(&old, &path).unwrap();
    assert!(matches!(
        ReceivedModTool.execute(&context, args, prepared).await,
        Err(AppError::Conflict(_))
    ));
}

#[tokio::test]
async fn rejects_html_with_an_archive_extension() {
    let (_dir, engine, _) = setup();
    let directory = incoming(&engine);
    fs::write(directory.join(FILENAME), b"<html>Download pending</html>").unwrap();
    let context = context(engine, ResolutionScope::Agent("fixture".into()));
    assert!(matches!(
        ReceivedModTool
            .prepare(&context, &json!({"gameId":"g","filename":FILENAME}))
            .await,
        Err(AppError::UnsafeArchive(_))
    ));
}

#[tokio::test]
async fn files_approved_archives_in_mods_without_installing_and_reuses_receipt() {
    let (_dir, engine, game) = setup();
    let incoming = incoming(&engine);
    let source = incoming.join(FILENAME);
    zip_at(&source, b"legitimate downloaded archive");
    let context = context(engine, ResolutionScope::Agent("fixture".into()));
    let folders = ReceiptFoldersTool
        .execute(
            &context,
            json!({}),
            ReceiptFoldersTool
                .prepare(&context, &json!({}))
                .await
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(folders["incoming"], paths::display_path(&incoming));
    assert_eq!(folders["playwright"], Value::Null);
    for (game_args, registered) in [
        (json!({"gameId":"g"}), true),
        (json!({"gameName":"Unregistered game"}), false),
    ] {
        let mut args = game_args;
        args["filename"] = json!(FILENAME);
        args["modName"] = json!("Fixture Mod");
        args["projectPage"] = json!("https://example.com/projects/fixture");
        let prepared = ReceivedModTool.prepare(&context, &args).await.unwrap();
        let sha256 = prepared.preview["sourceFingerprint"]["sha256"].clone();
        let entry = ReceivedModTool
            .execute(&context, args.clone(), prepared)
            .await
            .unwrap();
        assert_eq!(
            entry["gameId"],
            if registered { json!("g") } else { Value::Null }
        );
        assert_eq!(
            entry["gameName"],
            if registered {
                "Game"
            } else {
                "Unregistered game"
            }
        );
        assert_eq!(entry["name"], "Fixture Mod");
        assert_eq!(entry["projectPage"], "https://example.com/projects/fixture");
        assert_eq!(entry["sha256"], sha256);
        assert_eq!(entry["integrityVerified"], false);
        assert_eq!(entry["projectId"], Value::Null);
        assert_eq!(entry["versionId"], Value::Null);
        let archive = paths::safe_join(
            &context.engine.paths.root,
            entry["archiveRelativePath"].as_str().unwrap(),
        )
        .unwrap();
        assert!(archive.starts_with(context.engine.paths.mod_library()));
        assert_eq!(fs::read(archive).unwrap(), fs::read(&source).unwrap());
        let prepared = ReceivedModTool.prepare(&context, &args).await.unwrap();
        let repeat = ReceivedModTool
            .execute(&context, args, prepared)
            .await
            .unwrap();
        assert_eq!(repeat["artifactId"], entry["artifactId"]);
    }
    assert_eq!(
        mod_library::list_for_game(&context.engine.db, "g")
            .unwrap()
            .len(),
        1
    );
    assert!(fs::read_dir(game).unwrap().next().is_none());
}

#[tokio::test]
async fn files_stock_playwright_download_from_managed_inbox_into_mods() {
    let (_dir, engine, game) = setup_with_stock_playwright();
    let playwright = engine.paths.downloads().join("Playwright");
    assert!(
        playwright.is_dir(),
        "storage provisions downloads/Playwright"
    );
    let configured = mcp::playwright_receipt_dir(&engine.db).unwrap().unwrap();
    assert!(configured.is_absolute());
    assert_eq!(
        paths::display_path(&configured),
        paths::display_path(&playwright)
    );
    let source = playwright.join(FILENAME);
    zip_at(&source, b"completed browser download");
    let original = fs::read(&source).unwrap();
    let context = context(engine, ResolutionScope::Agent("fixture".into()));
    let lookup = json!({});
    let prepared = ReceiptFoldersTool.prepare(&context, &lookup).await.unwrap();
    let folders = ReceiptFoldersTool
        .execute(&context, lookup, prepared)
        .await
        .unwrap();
    assert_eq!(folders["playwright"], paths::display_path(&playwright));

    let args = json!({"gameId":"g","filename":FILENAME});
    let prepared = ReceivedModTool.prepare(&context, &args).await.unwrap();
    assert_eq!(prepared.preview["receiptFolder"], "playwright");
    assert_eq!(prepared.preview["sourcePath"], paths::display_path(&source));
    assert_eq!(
        prepared.preview["sourceFingerprint"]["sha256"],
        hex::encode(Sha256::digest(&original))
    );
    let entry = ReceivedModTool
        .execute(&context, args, prepared)
        .await
        .unwrap();
    let id = entry["artifactId"].as_str().unwrap();
    let verified = mod_library::verify_for_install(&context.engine, "g", id).unwrap();
    assert!(verified
        .archive_path
        .starts_with(context.engine.paths.mod_library()));
    assert_eq!(fs::read(verified.archive_path).unwrap(), original);
    assert_eq!(fs::read(source).unwrap(), original);
    assert_eq!(verified.entry.source_id, "local");
    assert!(!verified.entry.integrity_verified);
    assert!(verified.origin.is_none());
    assert!(fs::read_dir(game).unwrap().next().is_none());
}

#[tokio::test]
async fn right_magic_with_broken_archive_never_becomes_a_library_entry() {
    let (_dir, engine, _) = setup();
    let directory = incoming(&engine);
    fs::write(directory.join(FILENAME), b"PK\x03\x04invalid ZIP").unwrap();
    let context = context(engine, ResolutionScope::Agent("fixture".into()));
    let args = json!({"gameId":"g","filename":FILENAME});
    let prepared = ReceivedModTool.prepare(&context, &args).await.unwrap();
    assert!(matches!(
        ReceivedModTool.execute(&context, args, prepared).await,
        Err(AppError::UnsafeArchive(_))
    ));
    assert!(mod_library::list_for_game(&context.engine.db, "g")
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn received_download_preview_checks_entire_rar_signature() {
    let (_dir, engine, _) = setup();
    let directory = incoming(&engine);
    let context = context(engine, ResolutionScope::Agent("fixture".into()));
    for (filename, bytes, valid) in [
        ("legacy.rar", &b"Rar!\x1a\x07\x00"[..], true),
        ("current.RAR", &b"Rar!\x1a\x07\x01\x00"[..], true),
        ("short-legacy.rar", &b"Rar!\x1a\x07"[..], false),
        ("short-current.rar", &b"Rar!\x1a\x07\x01"[..], false),
    ] {
        fs::write(directory.join(filename), bytes).unwrap();
        let args = json!({"gameId":"g","filename":filename});
        let result = ReceivedModTool.prepare(&context, &args).await;
        if valid {
            // Preparation only verifies the signature and fingerprints the bytes.
            let preview = result.unwrap().preview;
            assert_eq!(preview["filename"], filename);
            assert_eq!(preview["sourceFingerprint"]["size"], bytes.len());
            assert_eq!(
                preview["sourceFingerprint"]["sha256"],
                hex::encode(Sha256::digest(bytes))
            );
        } else {
            assert!(matches!(result, Err(AppError::UnsafeArchive(_))));
        }
    }
    assert!(mod_library::list_for_game(&context.engine.db, "g")
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn accepts_playwright_spelled_targz_only_when_contents_are_tar() {
    use flate2::{write::GzEncoder, Compression};
    let (_dir, engine, _) = setup();
    let filename = "release-tar-1.gz";
    let source = incoming(&engine).join(filename);
    let encoder = GzEncoder::new(File::create(&source).unwrap(), Compression::default());
    let mut tar = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(4);
    header.set_mode(0o644);
    tar.append_data(&mut header, "content.txt", &b"test"[..])
        .unwrap();
    tar.into_inner().unwrap().finish().unwrap();
    let context = context(engine, ResolutionScope::Agent("fixture".into()));
    let args = json!({"gameId":"g","filename":filename});
    let prepared = ReceivedModTool.prepare(&context, &args).await.unwrap();
    let entry = ReceivedModTool
        .execute(&context, args, prepared)
        .await
        .unwrap();
    assert_eq!(entry["filename"], filename);

    let filename = "not-a-tar.gz";
    let mut encoder = GzEncoder::new(
        File::create(incoming(&context.engine).join(filename)).unwrap(),
        Compression::default(),
    );
    encoder
        .write_all(b"only a gzip stream, not a tar archive")
        .unwrap();
    encoder.finish().unwrap();
    let args = json!({"gameId":"g","filename":filename});
    let prepared = ReceivedModTool.prepare(&context, &args).await.unwrap();
    assert!(ReceivedModTool
        .execute(&context, args, prepared)
        .await
        .is_err());
    assert_eq!(
        mod_library::list_for_game(&context.engine.db, "g")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn receipt_target_respects_explicit_workspace_and_global_names() {
    let (_dir, engine, _) = setup();
    let game = games::get_game(&engine.db, "g").unwrap();
    let by_name = Args::parse(json!({"filename":FILENAME,"gameName":game.name})).unwrap();
    let selected = vec!["g".to_owned()];
    assert_eq!(
        by_name
            .target(&engine, Some(&selected))
            .unwrap()
            .id
            .as_deref(),
        Some("g")
    );
    assert!(by_name.target(&engine, Some(&[])).is_err());
    let multiple = vec!["g".into(), "other".into()];
    assert!(by_name.target(&engine, Some(&multiple)).is_err());
    let unknown =
        Args::parse(json!({"filename":FILENAME,"gameName":"Unregistered workspace test game"}))
            .unwrap();
    assert!(unknown.target(&engine, None).unwrap().id.is_none());
    assert!(unknown.target(&engine, Some(&selected)).is_err());
    let by_id = Args::parse(json!({"filename":FILENAME,"gameId":"g"})).unwrap();
    assert!(by_id.target(&engine, Some(&[])).is_err());
    assert!(by_id.target(&engine, Some(&["other".into()])).is_err());
    assert_eq!(
        by_id
            .target(&engine, Some(&multiple))
            .unwrap()
            .id
            .as_deref(),
        Some("g")
    );
}
