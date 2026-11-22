use super::*;
use crate::agent::policy::PermissionMode;
use std::{fs, io::Write};

const FILENAME: &str = "Release build.zip";

fn setup() -> (tempfile::TempDir, FileEngine, PathBuf) {
    let (dir, engine, game) = crate::transaction::tests::setup();
    // Keep all actual fixture file access within the temporary directory. The
    // stock Playwright launcher/path contract has separate config-level tests.
    let servers = mcp::McpServers::new(engine.db.clone());
    servers.seed_presets().unwrap();
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
    ] {
        assert!(archive_kind(filename).is_ok(), "{filename}");
    }
}

#[test]
fn chooses_only_actual_files_in_the_fixed_folders_and_rejects_ambiguity() {
    let (dir, engine, _) = setup();
    let incoming = incoming(&engine);
    let external = dir.path().canonicalize().unwrap().join("browser");
    fs::create_dir(&external).unwrap();
    let folders = Folders {
        incoming: incoming.clone(),
        playwright: Some(external.clone()),
    };
    zip_at(&engine.paths.downloads().join(FILENAME), b"not in inbox");
    zip_at(&dir.path().join(FILENAME), b"not in inbox");
    assert!(matches!(folders.find(FILENAME), Err(AppError::NotFound(_))));

    zip_at(&external.join(FILENAME), b"browser");
    let actual = folders.find(FILENAME).unwrap();
    assert!(matches!(actual.kind, Kind::Playwright));
    assert_eq!(actual.path, external.join(FILENAME));
    zip_at(&incoming.join(FILENAME), b"generic MCP");
    assert!(matches!(folders.find(FILENAME), Err(AppError::Conflict(_))));

    let other = incoming.join("directory.zip");
    fs::create_dir(&other).unwrap();
    assert!(folders.find("directory.zip").is_err());
    assert!(!Folders::playwright_allowed(
        &engine,
        &engine.paths.root.join("private")
    ));
}

#[test]
fn rejects_hard_links_and_symlinked_receipts_or_roots() {
    let (dir, engine, _) = setup();
    let incoming = incoming(&engine);
    let private = engine.paths.root.join("private");
    fs::create_dir(&private).unwrap();
    let source = private.join("secret.zip");
    zip_at(&source, b"private");
    fs::hard_link(&source, incoming.join(FILENAME)).unwrap();
    let folders = Folders {
        incoming,
        playwright: None,
    };
    assert!(folders.find(FILENAME).is_err());

    let link = dir.path().canonicalize().unwrap().join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&private, &link).unwrap();
    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(&private, &link).is_err() {
        return; // Windows may not grant Developer Mode / symlink privileges.
    }
    assert!(!Folders::playwright_allowed(&engine, &link));
    let folders = Folders {
        incoming: folders.incoming,
        playwright: Some(link),
    };
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
    for mode in [
        PermissionMode::RequestApproval,
        PermissionMode::AutoApprove,
        PermissionMode::FullTrust,
    ] {
        assert!(mode.needs_approval(ReceivedModTool.definition().effect));
    }
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
