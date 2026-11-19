use super::*;
use crate::{db::Db, settings::SecretStore, storage::AppPaths};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct NoSecrets;
impl SecretStore for NoSecrets {
    fn read(&self, _: &str) -> AppResult<Option<String>> {
        Ok(None)
    }
    fn write(&self, _: &str, _: &str) -> AppResult<()> {
        Ok(())
    }
}

struct CountingSecrets {
    reads: AtomicUsize,
    key: String,
}
impl SecretStore for CountingSecrets {
    fn read(&self, _: &str) -> AppResult<Option<String>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(Some(self.key.clone()))
    }
    fn write(&self, _: &str, _: &str) -> AppResult<()> {
        Ok(())
    }
}

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
        provider: "brave",
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
        || Ok(false),
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
        || Ok(true),
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
    let second = search_with(
        &engine,
        &root,
        &fp,
        "Reviewed Public Game",
        "SEARCH_GAME_IDENTITY",
        &CancellationToken::new(),
        || Ok(true),
        |_| async { panic!("Cache should prevent provider call") },
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
    for (fingerprint, confirmation, cancel) in [
        (fp.as_str(), "yes", CancellationToken::new()),
        ("wrong", "SEARCH_GAME_IDENTITY", CancellationToken::new()),
        (fp.as_str(), "SEARCH_GAME_IDENTITY", {
            let c = CancellationToken::new();
            c.cancel();
            c
        }),
    ] {
        assert!(search_with(
            &engine,
            &root,
            fingerprint,
            "Public Game",
            confirmation,
            &cancel,
            || Ok(false),
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
        || Ok(false),
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
            || Ok(false),
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

#[tokio::test]
async fn no_key_returns_real_configuration_state_even_with_cached_results() {
    let (_temp, engine, root) = fixture();
    let first = evidence(&engine, &root).await;
    let settings = Settings::with_secret_store(engine.db.clone(), Arc::new(NoSecrets));
    let searched = search(
        &engine,
        &settings,
        &root,
        &first.fingerprint,
        &first.query,
        "SEARCH_GAME_IDENTITY",
        &CancellationToken::new(),
    )
    .await
    .unwrap();
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
            || Ok(false),
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
async fn authorization_and_fingerprint_validation_precede_any_secret_access() {
    let (_temp, engine, root) = fixture();
    let fp = snapshot(&engine, &root).unwrap().fingerprint;
    let secrets = Arc::new(CountingSecrets {
        reads: AtomicUsize::new(0),
        key: "Public Game".into(),
    });
    let settings = Settings::with_secret_store(engine.db.clone(), secrets.clone());
    settings
        .write_secret(web_search::SECRET_REF, "ignored")
        .unwrap();
    let canceled = CancellationToken::new();
    canceled.cancel();
    for (fingerprint, confirmation, cancel) in [
        (fp.as_str(), "yes", CancellationToken::new()),
        (fp.as_str(), "SEARCH_GAME_IDENTITY", canceled),
        ("stale", "SEARCH_GAME_IDENTITY", CancellationToken::new()),
    ] {
        assert!(search(
            &engine,
            &settings,
            &root,
            fingerprint,
            "Public Game",
            confirmation,
            &cancel
        )
        .await
        .is_err());
        assert_eq!(secrets.reads.load(Ordering::SeqCst), 0);
    }
    fs::write(root.join("changed"), []).unwrap();
    assert!(search(
        &engine,
        &settings,
        &root,
        &fp,
        "Public Game",
        "SEARCH_GAME_IDENTITY",
        &CancellationToken::new()
    )
    .await
    .is_err());
    assert_eq!(secrets.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn raw_and_encoded_credentials_are_rejected_before_cache_or_network() {
    let (_temp, engine, root) = fixture();
    let e = evidence(&engine, &root).await;
    let secrets = Arc::new(CountingSecrets {
        reads: AtomicUsize::new(0),
        key: e.query.clone(),
    });
    let settings = Settings::with_secret_store(engine.db.clone(), secrets);
    settings
        .write_secret(web_search::SECRET_REF, "ignored")
        .unwrap();
    for query in [
        e.query.clone(),
        format!("find {} official", e.query),
        "Public%20Game%20video%20game".into(),
    ] {
        assert!(matches!(
            search(
                &engine,
                &settings,
                &root,
                &e.fingerprint,
                &query,
                "SEARCH_GAME_IDENTITY",
                &CancellationToken::new()
            )
            .await,
            Err(AppError::Denied(_))
        ));
    }
    assert!(load_evidence(&engine.db.conn(), &e.id).is_ok());
}
