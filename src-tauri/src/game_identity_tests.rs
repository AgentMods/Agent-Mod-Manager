use super::*;
use crate::{db::Db, storage::AppPaths};

fn fixture() -> (tempfile::TempDir, FileEngine, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(temp.path().join("manager")).unwrap();
    let db = Db::open(&paths.db_file()).unwrap();
    migrate(&db.conn()).unwrap();
    let engine = FileEngine::new(db, paths);
    let root = temp.path().join("Secret game directory");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("SecretGame.exe"), b"PRIVATE CONTENT NEVER READ").unwrap();
    (temp, engine, root)
}

fn result(status: WebSearchStatus) -> WebSearchResult {
    WebSearchResult {
        provider: web_search::PROVIDER,
        status,
        message: "Untrusted search excerpts",
        truncated: false,
        untrusted: true,
        results: vec![web_search::WebSearchHit {
            title: "Public Game - official site".into(),
            url: "https://store.steampowered.com/app/123/".into(),
            snippet: "An unverified excerpt".into(),
        }],
    }
}

async fn evidence(engine: &FileEngine, root: &Path) -> GameIdentitySearch {
    let fingerprint = snapshot(engine, root).unwrap().fingerprint;
    search_with(
        engine,
        root,
        &fingerprint,
        "Public Game video game",
        "SEARCH_GAME_IDENTITY",
        &CancellationToken::new(),
        |_| async { Ok(result(WebSearchStatus::Available)) },
    )
    .await
    .unwrap()
}

fn resign(e: &mut GameIdentitySearch) {
    e.id = format!("identity_{}", uuid::Uuid::new_v4().simple());
    e.id = format!(
        "{}_{}",
        e.id,
        hex::encode(Sha256::digest(serde_json::to_vec(e).unwrap()))
    );
}

fn replace_evidence(engine: &FileEngine, previous: &str, e: &GameIdentitySearch) {
    engine.db.conn().execute(
        "UPDATE game_identity_searches SET id=?1,root_path=?2,fingerprint=?3,query=?4,evidence_json=?5,created_at=?6 WHERE id=?7",
        params![e.id,e.root_path,e.fingerprint,e.query,serde_json::to_string(e).unwrap(),e.created_at,previous],
    ).unwrap();
}

#[test]
fn preview_only_suggests_an_adapter_after_its_package_is_installed() {
    let (_temp, engine, root) = fixture();
    fs::write(root.join("SkyrimSE.exe"), b"fixture").unwrap();
    fs::create_dir(root.join("Data")).unwrap();
    fs::write(root.join("Data/Skyrim.esm"), b"fixture").unwrap();

    let without_package = preview(&engine, &root).unwrap();
    assert!(without_package.local_candidate.matches.is_empty());
    assert!(without_package
        .local_candidate
        .suggested_adapter_slug
        .is_none());

    crate::plugins::test_fixtures::install(&engine.db, &engine.paths).unwrap();
    let with_package = preview(&engine, &root).unwrap();
    assert_eq!(with_package.fingerprint, without_package.fingerprint);
    assert_eq!(
        with_package
            .local_candidate
            .suggested_adapter_slug
            .as_deref(),
        Some("skyrim-se")
    );
    assert!(games::list_games(&engine.db).unwrap().is_empty());
}

#[test]
fn preview_is_read_only_bounded_and_shallow() {
    let (_temp, engine, root) = fixture();
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join("nested/private.txt"), b"Not scanned").unwrap();
    let p = preview(&engine, &root).unwrap();
    assert!(p.signals.iter().any(|s| s == "file: SecretGame.exe"));
    assert!(!p
        .signals
        .iter()
        .any(|s| s.contains("private.txt") || s.contains("PRIVATE CONTENT")));
    assert_eq!(p.fingerprint, preview(&engine, &root).unwrap().fingerprint);
    assert_eq!(p.local_candidate.confidence, 0.0);
    assert!(games::list_games(&engine.db).unwrap().is_empty());
    assert!(snapshot(&engine, Path::new("/")).is_err());
    assert!(snapshot(&engine, &engine.paths.root).is_err());
    assert!(snapshot(&engine, engine.paths.root.parent().unwrap()).is_err());
    assert!(snapshot(&engine, &engine.paths.downloads()).is_err());
    assert!(snapshot(&engine, Path::new("/etc")).is_err());
    assert!(snapshot(&engine, Path::new("relative")).is_err());
    for i in 0..MAX_ENTRIES {
        fs::write(root.join(format!("entry{i}")), []).unwrap();
    }
    assert!(snapshot(&engine, &root).is_err());
}

#[cfg(unix)]
#[test]
fn links_special_files_and_invalid_names_are_not_signals() {
    use std::os::unix::fs::symlink;
    let (temp, engine, root) = fixture();
    symlink("/etc/passwd", root.join("linked-private-file")).unwrap();
    symlink("/etc", root.join("linked-system-directory")).unwrap();
    let socket = std::os::unix::net::UnixListener::bind(root.join("socket")).unwrap();
    let p = preview(&engine, &root).unwrap();
    assert_eq!(p.signals.len(), 1);
    symlink(&root, temp.path().join("alias")).unwrap();
    assert!(snapshot(&engine, &temp.path().join("alias")).is_err());
    drop(socket);
    fs::write(root.join("bad\nname"), []).unwrap();
    assert!(snapshot(&engine, &root).is_err());
    fs::remove_file(root.join("bad\nname")).unwrap();
    // APFS rejects non-UTF-8 names at creation; Linux permits this fixture.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStringExt;
        fs::write(root.join(std::ffi::OsString::from_vec(vec![0xff])), []).unwrap();
        assert!(snapshot(&engine, &root).is_err());
    }
}

#[test]
fn names_and_queries_reject_paths_controls_and_oversize() {
    for name in [
        "",
        " ",
        "..",
        "/etc/passwd",
        "C:\\Game",
        "C:",
        "C:private",
        "bad\nname",
        "bad\u{202e}name",
        "bad\u{2028}name",
        "\u{061c}",
        " trailing ",
    ] {
        assert!(validate_name(name).is_err(), "{name:?}");
    }
    assert!(validate_name(&"x".repeat(129)).is_err());
    assert!(validate_name("游戏名称").is_ok());
    assert!(validate_name("NieR:Automata").is_ok());
    assert!(validate_name("The Elder Scrolls V: Skyrim").is_ok());
    assert!(validate_name("X: Beyond the Frontier").is_ok());
    for query in [
        "/Users/private/game",
        "C:\\private\\game",
        "file:///tmp/private",
        "bad\nquery",
    ] {
        assert!(public_query(query).is_err());
    }
    assert!(public_query(&"x".repeat(513)).is_err());
    assert!(public_query("Public Game official website").is_ok());
}

#[tokio::test]
async fn provider_receives_only_reviewed_query_and_cache_roundtrips() {
    let (_temp, engine, root) = fixture();
    let fp = snapshot(&engine, &root).unwrap().fingerprint;
    let first = search_with(
        &engine,
        &root,
        &fp,
        "Reviewed Public Game",
        "SEARCH_GAME_IDENTITY",
        &CancellationToken::new(),
        |q| async move {
            assert_eq!(q.query, "Reviewed Public Game");
            assert_eq!(q.count, 5);
            assert!(!q.query.contains("Secret"));
            let mut r = result(WebSearchStatus::Available);
            r.results.resize(9, r.results[0].clone());
            Ok(r)
        },
    )
    .await
    .unwrap();
    assert_eq!(first.candidates.len(), 5);
    assert_eq!(serde_json::to_value(&first).unwrap()["provider"], "bing");
    assert!(search(
        &engine,
        &McpRuntime::default(),
        &root,
        &fp,
        "Reviewed Public Game",
        "yes",
        &CancellationToken::new(),
    )
    .await
    .is_err());
    let second = search(
        &engine,
        &McpRuntime::default(),
        &root,
        &fp,
        "Reviewed Public Game",
        "SEARCH_GAME_IDENTITY",
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(load_evidence(&engine.db.conn(), &first.id).unwrap()).unwrap()
    );
    let other = evidence(&engine, &root).await;
    assert_ne!(first.id, other.id);
    assert!(confirm(&engine, &first.id, 0, "Name", "CONFIRM_GAME_IDENTITY").is_err());
}

#[tokio::test]
async fn consent_stale_preview_and_cancellation_stop_before_provider() {
    let (_temp, engine, root) = fixture();
    let fp = snapshot(&engine, &root).unwrap().fingerprint;
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    for (fingerprint, query, confirmation, cancel) in [
        (fp.as_str(), "Public Game", "yes", CancellationToken::new()),
        (
            "wrong",
            "Public Game",
            "SEARCH_GAME_IDENTITY",
            CancellationToken::new(),
        ),
        (
            fp.as_str(),
            "/private/file.exe",
            "SEARCH_GAME_IDENTITY",
            CancellationToken::new(),
        ),
        (
            fp.as_str(),
            "Public Game",
            "SEARCH_GAME_IDENTITY",
            cancelled,
        ),
    ] {
        assert!(search_with(
            &engine,
            &root,
            fingerprint,
            query,
            confirmation,
            &cancel,
            |_| async { panic!("Provider must not run") }
        )
        .await
        .is_err());
    }
    assert!(search_with(
        &engine,
        &root,
        &fp,
        "Public Game",
        "SEARCH_GAME_IDENTITY",
        &CancellationToken::new(),
        |_| async {
            fs::write(root.join("changed"), []).unwrap();
            Ok(result(WebSearchStatus::Available))
        }
    )
    .await
    .is_err());
    assert!(games::list_games(&engine.db).unwrap().is_empty());
}

#[tokio::test]
async fn cancelled_provider_does_not_store_evidence() {
    let (_temp, engine, root) = fixture();
    let fp = snapshot(&engine, &root).unwrap().fingerprint;
    let cancel = CancellationToken::new();
    assert!(matches!(
        search_with(
            &engine,
            &root,
            &fp,
            "Public Game",
            "SEARCH_GAME_IDENTITY",
            &cancel,
            |_| async {
                cancel.cancel();
                Ok(result(WebSearchStatus::Available))
            }
        )
        .await,
        Err(AppError::Cancelled)
    ));
    let count: i64 = engine
        .db
        .conn()
        .query_row("SELECT COUNT(*) FROM game_identity_searches", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}

// Write the old JSON layout independently of the current serializer: its byte order
// is part of the ID digest, and old records did not have a provider field.
fn store_legacy_brave_evidence(engine: &FileEngine, root: &Path) -> GameIdentitySearch {
    let scan = snapshot(engine, root).unwrap();
    let root = scan.root.to_str().unwrap();
    let query = "Public Game video game";
    let created_at = Utc::now().to_rfc3339();
    let encoded = |id: &str| {
        format!(
            concat!(
                r#"{{"id":"{id}","rootPath":{root},"fingerprint":"{fingerprint}","query":"{query}","status":"available","message":"Brave search excerpts","candidates":[{{"name":"Public Game","title":"Brave result","url":"https://store.steampowered.com/app/123/","snippet":"An unverified excerpt"}}],"createdAt":"{created_at}"}}"#,
            ),
            id = id,
            root = serde_json::to_string(root).unwrap(),
            fingerprint = scan.fingerprint,
            query = query,
            created_at = created_at,
        )
    };
    let prefix = format!("identity_{}", uuid::Uuid::new_v4().simple());
    let digest = hex::encode(Sha256::digest(encoded(&prefix)));
    let id = format!("{prefix}_{digest}");
    let conn = engine.db.conn();
    conn.execute(
        "INSERT INTO game_identity_searches(id,root_path,fingerprint,query,evidence_json,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
        params![id, root, scan.fingerprint, query, encoded(&id), created_at],
    )
    .unwrap();
    load_evidence(&conn, &id).unwrap()
}

#[test]
fn historical_evidence_confirmed_under_original_id_retains_brave_source() {
    let (_temp, engine, root) = fixture();
    let legacy = store_legacy_brave_evidence(&engine, &root);
    assert!(legacy.provider.is_none());
    let game = confirm(&engine, &legacy.id, 0, "Name", "CONFIRM_GAME_IDENTITY").unwrap();
    assert_eq!(game.metadata["onlineIdentity"]["source"], "brave");
    assert_eq!(game.metadata["onlineIdentity"]["searchId"], legacy.id);
    assert_eq!(game.metadata["onlineIdentity"]["title"], "Brave result");
}

#[tokio::test]
async fn bing_lookup_does_not_reuse_brave_cache_and_reports_missing_runtime() {
    let (_temp, engine, root) = fixture();
    let legacy = store_legacy_brave_evidence(&engine, &root);
    let searched = search(
        &engine,
        &McpRuntime::default(),
        &root,
        &legacy.fingerprint,
        &legacy.query,
        "SEARCH_GAME_IDENTITY",
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_ne!(searched.id, legacy.id);
    assert_eq!(searched.evidence_provider(), "bing");
    assert_eq!(searched.status, GameIdentityStatus::NeedsConfiguration);
    assert!(searched.candidates.is_empty());
    assert!(confirm(&engine, &searched.id, 0, "Name", "CONFIRM_GAME_IDENTITY").is_err());
}

#[tokio::test]
async fn confirm_is_atomic_preserves_metadata_counts_and_never_guesses_steam_or_adapter() {
    let (_temp, engine, root) = fixture();
    let game = games::register_game(&engine.db, &root, None).unwrap();
    engine.db.conn().execute(r#"UPDATE games SET installed_mod_count=7,enabled_mod_count=3,metadata_json='{"custom":42}',cover_path='cover' WHERE id=?1"#, [&game.id]).unwrap();
    let searched = evidence(&engine, &root).await;
    assert!(confirm(&engine, &searched.id, 0, "User reviewed name", "yes").is_err());
    assert!(confirm(
        &engine,
        &searched.id,
        9,
        "User reviewed name",
        "CONFIRM_GAME_IDENTITY"
    )
    .is_err());
    let confirmed = confirm(
        &engine,
        &searched.id,
        0,
        "User reviewed name",
        "CONFIRM_GAME_IDENTITY",
    )
    .unwrap();
    assert_eq!(confirmed.id, game.id);
    assert_eq!(confirmed.name, "User reviewed name");
    assert_eq!(
        (confirmed.installed_mod_count, confirmed.enabled_mod_count),
        (7, 3)
    );
    assert_eq!(confirmed.cover_path.as_deref(), Some("cover"));
    assert_eq!(confirmed.metadata["custom"], 42);
    assert_eq!(confirmed.metadata["onlineIdentity"]["source"], "bing");
    assert_eq!(
        confirmed.metadata["onlineIdentity"]["query"],
        searched.query
    );
    assert_eq!(
        confirmed.metadata["onlineIdentity"]["url"],
        searched.candidates[0].url
    );
    assert!(confirmed.adapter_slug.is_none());
    assert!(confirmed.steam_app_id.is_none());
    assert!(confirmed.confidence <= 0.6);
    assert_eq!(confirmed.created_at, game.created_at);
    assert!(confirm(&engine, &searched.id, 0, "Name", "CONFIRM_GAME_IDENTITY").is_err());
}

#[tokio::test]
async fn adapter_and_pending_recovery_block_registration_without_consuming_evidence() {
    let (_temp, engine, root) = fixture();
    let searched = evidence(&engine, &root).await;
    engine.db.conn().execute("INSERT INTO file_transactions(id,kind,status,plan_json,created_at) VALUES('pending','test','recovery_failed','{}','now')", []).unwrap();
    assert!(confirm(&engine, &searched.id, 0, "Name", "CONFIRM_GAME_IDENTITY").is_err());
    assert!(games::list_games(&engine.db).unwrap().is_empty());
    engine
        .db
        .conn()
        .execute("DELETE FROM file_transactions", [])
        .unwrap();
    let game = games::register_game(&engine.db, &root, None).unwrap();
    engine
        .db
        .conn()
        .execute(
            "UPDATE games SET adapter_slug='confirmed',confidence=1 WHERE id=?1",
            [&game.id],
        )
        .unwrap();
    assert!(confirm(&engine, &searched.id, 0, "Name", "CONFIRM_GAME_IDENTITY").is_err());
    assert_eq!(
        games::get_game(&engine.db, &game.id).unwrap().name,
        game.name
    );
    assert!(load_evidence(&engine.db.conn(), &searched.id).is_ok());
}

#[tokio::test]
async fn expired_future_tampered_and_directory_changed_evidence_is_rejected() {
    let (_temp, engine, root) = fixture();
    for delta in [-TTL_SECONDS - 1, 60] {
        let mut e = evidence(&engine, &root).await;
        let previous = e.id.clone();
        e.created_at = (Utc::now() + chrono::Duration::seconds(delta)).to_rfc3339();
        resign(&mut e);
        replace_evidence(&engine, &previous, &e);
        assert!(confirm(&engine, &e.id, 0, "Name", "CONFIRM_GAME_IDENTITY").is_err());
    }
    let mut e = evidence(&engine, &root).await;
    let previous = e.id.clone();
    e.candidates[0].title = "Tampered title".into();
    replace_evidence(&engine, &previous, &e);
    assert!(confirm(&engine, &e.id, 0, "Name", "CONFIRM_GAME_IDENTITY").is_err());
    let e = evidence(&engine, &root).await;
    fs::write(root.join("new-file"), []).unwrap();
    assert!(confirm(&engine, &e.id, 0, "Name", "CONFIRM_GAME_IDENTITY").is_err());
    assert!(games::list_games(&engine.db).unwrap().is_empty());
}

#[tokio::test]
async fn empty_available_and_unavailable_results_are_not_confirmable() {
    let (_temp, engine, root) = fixture();
    let fp = snapshot(&engine, &root).unwrap().fingerprint;
    for status in [WebSearchStatus::Available, WebSearchStatus::Unavailable] {
        let e = search_with(
            &engine,
            &root,
            &fp,
            "Public Game",
            "SEARCH_GAME_IDENTITY",
            &CancellationToken::new(),
            |_| async {
                let mut r = result(status);
                r.results.clear();
                Ok(r)
            },
        )
        .await
        .unwrap();
        assert!(confirm(&engine, &e.id, 0, "Name", "CONFIRM_GAME_IDENTITY").is_err());
    }
}

#[tokio::test]
async fn failed_evidence_consumption_rolls_back_game_registration() {
    let (_temp, engine, root) = fixture();
    let e = evidence(&engine, &root).await;
    engine.db.conn().execute_batch("CREATE TRIGGER fail_identity_consume BEFORE UPDATE OF invalidated ON game_identity_searches BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
    assert!(confirm(&engine, &e.id, 0, "Name", "CONFIRM_GAME_IDENTITY").is_err());
    assert!(games::list_games(&engine.db).unwrap().is_empty());
    assert!(load_evidence(&engine.db.conn(), &e.id).is_ok());
    engine
        .db
        .conn()
        .execute_batch("DROP TRIGGER fail_identity_consume")
        .unwrap();
    let game = confirm(&engine, &e.id, 0, "Name", "CONFIRM_GAME_IDENTITY").unwrap();
    assert_eq!(game.name, "Name");
    assert!(game.adapter_slug.is_none());
}

#[test]
fn fingerprint_binds_root_and_file_metadata_not_enumeration_order() {
    let (_temp, engine, root) = fixture();
    let before = snapshot(&engine, &root).unwrap().fingerprint;
    fs::write(root.join("SecretGame.exe"), b"DIFFERENT SIZE").unwrap();
    assert_ne!(snapshot(&engine, &root).unwrap().fingerprint, before);
    let other = root.with_file_name("OtherGame");
    fs::rename(&root, &other).unwrap();
    assert_ne!(snapshot(&engine, &other).unwrap().fingerprint, before);
}

#[test]
fn standard_mac_steam_and_minecraft_roots_have_exact_bounded_exceptions() {
    let (temp, engine, _) = fixture();
    let home = temp.path().join("home");
    let library = home.join("Library");
    let common = library.join("Application Support/Steam/steamapps/common");
    let game = common.join("Specific Game");
    fs::create_dir_all(&game).unwrap();
    fs::write(game.join("game.exe"), []).unwrap();
    assert!(known_user_game(&game, &library, true));
    assert!(!sensitive_user_path(&game, &home));
    assert!(preview(&engine, &game).is_ok());
    for denied in [
        library.clone(),
        library.join("Keychains"),
        library.join("Application Support"),
        common.clone(),
        common.parent().unwrap().to_owned(),
        game.join("nested"),
        library.join("Application Support/Steam/steamapps/commonish/Fake Game"),
    ] {
        assert!(sensitive_user_path(&denied, &home), "{denied:?}");
    }
    assert!(!sensitive_user_path(
        &library.join("Application Support/minecraft"),
        &home
    ));
    assert!(sensitive_user_path(
        &library.join("Application Support/minecraft/private"),
        &home
    ));
    let appdata = home.join("AppData/Roaming");
    assert!(known_user_game(
        &appdata.join(".minecraft"),
        &appdata,
        false
    ));
    assert!(!known_user_game(&appdata, &appdata, false));
    assert!(!known_user_game(
        &appdata.join(".minecraft/other"),
        &appdata,
        false
    ));
    assert!(!known_user_game(
        &appdata.join("Secret App"),
        &appdata,
        false
    ));
    assert!(known_user_game(
        &appdata.join("Steam/steamapps/common/Specific Game"),
        &appdata,
        false
    ));
    assert!(!known_user_game(
        &appdata.join("Steam/steamapps/common"),
        &appdata,
        false
    ));
}

#[tokio::test]
async fn removing_bing_provenance_cannot_convert_evidence_to_brave() {
    let (_temp, engine, root) = fixture();
    let mut searched = evidence(&engine, &root).await;
    let id = searched.id.clone();
    searched.provider = None;
    replace_evidence(&engine, &id, &searched);
    assert!(matches!(
        confirm(&engine, &id, 0, "Name", "CONFIRM_GAME_IDENTITY"),
        Err(AppError::Invalid(message)) if message == "Search evidence was modified"
    ));
    assert!(games::list_games(&engine.db).unwrap().is_empty());
}
