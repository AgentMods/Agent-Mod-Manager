use super::*;
use crate::{
    history::History,
    sources::{FileHashes, ResolveResult, ResolvedMod, SourceState, SourceStatus},
    transaction::tests::setup,
};
use std::{fs, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn publish(resolutions: &Resolutions, address: std::net::SocketAddr) -> uuid::Uuid {
    resolutions
        .publish(
            ResolutionScope::Desktop,
            ResolveResult {
                status: SourceStatus::new("modrinth", SourceState::Available, "OK"),
                download: Some(ResolvedMod {
                    source_id: "modrinth".into(),
                    project_id: "example-project".into(),
                    version_id: "1.2.3".into(),
                    name: "Example Mod".into(),
                    version: "1.2.3".into(),
                    download_url: format!("http://{address}/mod.zip"),
                    filename: "mod.zip".into(),
                    size: Some(22),
                    hashes: FileHashes::default(),
                    homepage_url: None,
                    warnings: Vec::new(),
                }),
                homepage_url: None,
                dependencies: None,
            },
        )
        .unwrap()
        .download
        .unwrap()
        .resolution_id
}

#[tokio::test]
async fn desktop_can_retry_failed_ingest_without_network_or_another_resolution() {
    let (_dir, engine, _) = setup();
    let resolutions = Resolutions::default();
    let tasks = DownloadTasks::default();
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let id = publish(&resolutions, listener.local_addr().unwrap());
    let progress: ProgressCallback = Arc::new(|_| ());
    assert!(matches!(
        tasks
            .run(
                &engine,
                &resolutions,
                "download-preflight".into(),
                id,
                ClassificationInput::default(),
                progress.clone()
            )
            .await,
        Err(AppError::Invalid(_))
    ));
    assert!(resolutions.preview(&ResolutionScope::Desktop, id).is_ok());
    assert!(crate::artifacts::list(&engine.db).unwrap().is_empty());

    let target = ClassificationInput {
        game_name: Some("Unregistered Game".into()),
        author: Some("Public Archive Author".into()),
        project_page: Some("https://example.com/archive-project".into()),
        ..Default::default()
    };
    let root = engine.paths.mod_library();
    fs::remove_dir(&root).unwrap();
    fs::write(&root, b"blocked Mods library").unwrap();
    const ARCHIVE: [u8; 22] = [
        0x50, 0x4b, 5, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            tasks.run(
                &engine,
                &resolutions,
                "download-mock".into(),
                id,
                target.clone(),
                progress
            ),
            async {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    header.push(socket.read_u8().await.unwrap());
                    assert!(header.len() < 16 * 1024);
                }
                assert!(header.starts_with(b"GET /mod.zip "));
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 22\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
                socket.write_all(&ARCHIVE).await.unwrap();
            }
        )
    })
    .await
    .unwrap();
    let error = result.unwrap_err().to_string();
    let records = crate::artifacts::list(&engine.db).unwrap();
    let [saved] = records.as_slice() else {
        panic!("expected a durable transport receipt")
    };
    assert!(error.contains(&saved.artifact.id));
    assert!(resolutions.preview(&ResolutionScope::Desktop, id).is_err());
    assert_eq!(fs::read(&saved.artifact.path).unwrap(), ARCHIVE);

    fs::remove_file(&root).unwrap();
    fs::create_dir(&root).unwrap();
    let wrong_game = ClassificationInput {
        game_name: Some("Not the approved game".into()),
        ..Default::default()
    };
    assert!(matches!(
        tasks.retry(&engine, &saved.artifact.id, wrong_game).await,
        Err(AppError::Conflict(_))
    ));
    assert!(matches!(
        tasks
            .retry(&engine, &saved.artifact.id, target.clone())
            .await,
        Err(AppError::Invalid(_))
    ));
    let target = ClassificationInput {
        game_name: target.game_name,
        ..Default::default()
    };
    let result = tasks
        .retry(&engine, &saved.artifact.id, target.clone())
        .await
        .unwrap();
    assert!(result.artifact.path.starts_with(&root));
    assert_eq!(fs::read(&result.artifact.path).unwrap(), ARCHIVE);
    let stored = crate::artifacts::for_id(&engine.db, &saved.artifact.id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.artifact.path, result.artifact.path);
    assert!(
        !saved.artifact.path.exists(),
        "successful native retry must not keep another archive in the transport cache"
    );
    assert_eq!(result.artifact_id, saved.artifact.id);
    assert_eq!(result.game_name, "Unregistered Game");
    assert!(result.game_id.is_none());
    let wire = json!(result.clone().into_user_paths());
    assert_eq!(wire["id"], saved.artifact.id);
    assert_eq!(wire["artifactId"], saved.artifact.id);
    assert_eq!(wire["projectPage"], "https://example.com/archive-project");
    assert_eq!(wire["authorHint"], "Public Archive Author");
    let (page, author): (String, String) = engine.db.conn().query_row(
        "SELECT project_page,author_hint FROM mod_library_entries WHERE id=?1 AND state='ready'",
        [&saved.artifact.id], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(page, "https://example.com/archive-project");
    assert_eq!(author, "Public Archive Author");
    assert_eq!(
        wire["path"],
        json!(crate::paths::user_path(&result.artifact.path))
    );
    let repeated = tasks
        .retry(&engine, &saved.artifact.id, target)
        .await
        .unwrap();
    assert_eq!(repeated.artifact.path, result.artifact.path);
    assert_eq!(repeated.project_page, result.project_page);
    assert_eq!(repeated.author_hint, result.author_hint);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
}

#[test]
fn download_target_uses_registered_chat_game_and_rejects_conflicting_selection() {
    let (_dir, engine, _) = setup();
    let full_name = "游戏显示名称".repeat(200);
    engine
        .db
        .conn()
        .execute("UPDATE games SET name=?1 WHERE id='g'", [&full_name])
        .unwrap();
    let explicit = ClassificationInput {
        game_name: Some(full_name.clone()),
        ..Default::default()
    };
    assert_eq!(
        explicit.resolve_target(&engine).unwrap().game_name,
        full_name
    );
    let chat = History::new(engine.db.clone())
        .create_conversation("Mod download", Some("g"))
        .unwrap();
    let context = crate::agent::tools::ToolContext {
        engine,
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: ResolutionScope::Agent(chat.id),
    };
    let target = ClassificationInput::default().for_agent(&context).unwrap();
    assert_eq!(target.game_id.as_deref(), Some("g"));
    assert_eq!(target.game_name, full_name);
    assert!(ClassificationInput {
        game_name: Some("Other Game".into()),
        ..Default::default()
    }
    .for_agent(&context)
    .is_err());
}

#[test]
fn workspace_downloads_require_an_explicit_member_of_multi_and_never_assume_empty_is_all() {
    let (dir, engine, _) = setup();
    engine.db.conn().execute(
        "INSERT INTO games(id,name,slug,root_path,created_at,updated_at) VALUES('other','Other','other',?1,'now','now')",
        [dir.path().join("second").to_str().unwrap()],
    ).unwrap();
    let history = History::new(engine.db.clone());
    let chat = history.create_conversation("download", None).unwrap();
    let ctx = crate::agent::tools::ToolContext {
        engine,
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: ResolutionScope::Agent(chat.id.clone()),
    };
    let by_name = || ClassificationInput {
        game_name: Some("Unregistered".into()),
        ..Default::default()
    };
    let by_id = |id: &str| ClassificationInput {
        game_id: Some(id.into()),
        ..Default::default()
    };
    assert!(by_name().for_agent(&ctx).is_ok()); // Legacy dynamic-all conversations.
    history
        .set_conversation_workspace(&chat.id, Some(&["g".into(), "other".into()]))
        .unwrap();
    assert!(ClassificationInput::default().for_agent(&ctx).is_err());
    assert!(by_name().for_agent(&ctx).is_err());
    assert_eq!(
        by_id("other").for_agent(&ctx).unwrap().game_id.as_deref(),
        Some("other")
    );
    history
        .set_conversation_workspace(&chat.id, Some(&[]))
        .unwrap();
    assert!(by_id("g").for_agent(&ctx).is_err());
    assert!(by_name().for_agent(&ctx).is_err());
}

#[test]
fn metadata_that_resolves_to_a_transport_url_is_not_a_project_page() {
    for (project, transfer) in [
        (
            "https://example.com/opaque",
            "https://example.com/opaque?signature=private",
        ),
        (
            "https://EXAMPLE.com:443/opaque",
            "https://example.com/opaque",
        ),
    ] {
        assert!(artifacts::same_public_endpoint(project, transfer));
        let mut target = DownloadTarget {
            game_name: "Game".into(),
            game_id: None,
            author: None,
            project_page: Some(project.into()),
        };
        target.omit_transport_page(transfer);
        assert!(target.project_page.is_none());
    }
    assert!(!artifacts::same_public_endpoint(
        "https://example.com/project",
        "https://example.com/opaque?signature=private"
    ));
}
