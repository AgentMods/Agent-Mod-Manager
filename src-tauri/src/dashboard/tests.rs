use super::*;
use serde_json::{json, Value};

fn db() -> (tempfile::TempDir, Db) {
    let temp = tempfile::tempdir().unwrap();
    let db = Db::open(&temp.path().join("dashboard.sqlite")).unwrap();
    (temp, db)
}

fn register(db: &Db, id: &str, name: &str) {
    db.conn()
        .execute(
            "INSERT INTO games (id,name,slug,root_path,installed_mod_count,created_at,updated_at)
         VALUES (?1,?2,?1,?1,99,'now','now')",
            params![id, name],
        )
        .unwrap();
}

fn archive(db: &Db, id: &str, game_id: Option<&str>, game_name: &str, state: &str) {
    let conn = db.conn();
    conn.execute(
        "INSERT OR IGNORE INTO mod_library_categories VALUES ('demo','demo')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO mod_library_entries (id,category_key,game_id,game_name,game_name_key,name,
            filename,archive_relative_path,payload_relative_path,size,sha256,source_id,version,
            integrity_verified,created_at,state)
         VALUES (?1,'demo',?2,?3,?4,?1,?1,?1,?1,0,'hash','web','1',0,'now',?5)",
        params![
            id,
            game_id,
            game_name,
            mod_library::game_key(game_name),
            state
        ],
    )
    .unwrap();
}

fn record(db: &Db, id: &str, game_id: &str, status: &str, metadata: Value) {
    db.conn().execute(
        "INSERT INTO installs (id,game_id,name,status,install_root,metadata_json,installed_at,updated_at)
         VALUES (?1,?2,?1,?3,'unused',?4,'now','now')",
        params![id, game_id, status, metadata.to_string()],
    ).unwrap();
}

fn hide(db: &Db, game_id: &str, kind: &str, id: &str) {
    db.conn()
        .execute(
            "INSERT INTO removed_mod_items VALUES (?1,?2,?3)",
            params![game_id, kind, id],
        )
        .unwrap();
}

fn origin(id: &str) -> Value {
    json!({ "artifactId": id, "sourceId": "provider", "projectId": "project", "versionId": "version" })
}

#[test]
fn empty_registered_library_returns_zero_counts_in_ipc_shape() {
    let (_temp, db) = db();
    archive(&db, "not-registered", None, "Demo", "ready");
    record(&db, "orphan", "deleted", "enabled", json!({}));
    assert_eq!(
        serde_json::to_value(read_stats(&db).unwrap()).unwrap(),
        json!({ "managedGameCount": 0, "installedModCount": 0, "totalModCount": 0 })
    );
}

#[test]
fn counts_visible_library_rows_per_registration_and_not_unique_archive_ids() {
    let (_temp, db) = db();
    register(&db, "first", "Démo  Game");
    register(&db, "second", "DÉMO GAME");
    register(&db, "no-mods-no-adapter", "Other");

    for id in [
        "shared",
        "history-archive",
        "fallback-archive",
        "invalid-origin-archive",
    ] {
        archive(&db, id, None, "démo game", "ready");
    }
    archive(
        &db,
        "archive-only",
        Some("first"),
        "Old displayed name",
        "ready",
    );
    archive(&db, "hidden-archive", Some("first"), "Démo Game", "ready");
    archive(
        &db,
        "pending-archive",
        Some("first"),
        "Démo Game",
        "pending",
    );
    archive(&db, "orphan-archive", Some("deleted"), "Démo Game", "ready");

    record(
        &db,
        "active",
        "first",
        "enabled",
        json!({"libraryEntryId":"shared", "downloadOrigin":origin("archive-only")}),
    );
    record(&db, "legacy", "first", "installed", json!({}));
    record(
        &db,
        "history",
        "first",
        "uninstalled",
        json!({"libraryEntryId":"history-archive"}),
    );
    record(
        &db,
        "replaced",
        "first",
        "superseded",
        json!({"libraryEntryId":"history-archive"}),
    );
    record(
        &db,
        "partial",
        "first",
        "partially_restored",
        json!({"libraryEntryId":null,"downloadOrigin":origin("fallback-archive")}),
    );
    record(
        &db,
        "plan",
        "first",
        "planned",
        json!({"downloadOrigin":{"artifactId":"invalid-origin-archive"}}),
    );
    record(
        &db,
        "dismissed",
        "first",
        "rolled_back",
        json!({"libraryEntryId":"archive-only"}),
    );
    record(&db, "dismissed-plan", "first", "planned", json!({}));
    hide(&db, "first", "install", "dismissed");
    hide(&db, "first", "install", "dismissed-plan");
    hide(&db, "first", "archive", "hidden-archive");
    hide(&db, "second", "archive", "history-archive");

    assert_eq!(
        read_stats(&db).unwrap(),
        DashboardStats {
            managed_game_count: 3,
            installed_mod_count: 2,
            total_mod_count: 11, // First: six records and two archives. Second: three archives.
        }
    );
}

#[test]
fn reads_journal_effective_status_without_reconciling_persisted_records() {
    let (_temp, db) = db();
    register(&db, "demo", "Demo");
    archive(&db, "restored-archive", Some("demo"), "Demo", "ready");
    archive(&db, "old-archive", Some("demo"), "Demo", "ready");
    record(
        &db,
        "restored",
        "demo",
        "uninstalled",
        json!({"libraryEntryId":"restored-archive"}),
    );
    record(&db, "rolled-back", "demo", "enabled", json!({}));
    record(&db, "waiting", "demo", "enabled", json!({}));
    record(
        &db,
        "old",
        "demo",
        "installed",
        json!({"downloadOrigin":{"artifactId":"old-archive"}}),
    );
    hide(&db, "demo", "install", "restored");
    hide(&db, "demo", "archive", "restored-archive");
    hide(&db, "demo", "install", "old");
    hide(&db, "demo", "archive", "old-archive");
    db.conn()
        .execute_batch(
            "INSERT INTO file_transactions (id,kind,status,game_id,plan_json,created_at) VALUES
            ('apply','install','committed','demo','{}','now'),
            ('undo','install','rolled_back','demo','{}','now'),
            ('plan','install','planned','demo','{}','now');
         INSERT INTO install_transactions (install_id,transaction_id,action,created_at) VALUES
            ('restored','apply','install','now'),('rolled-back','undo','install','now'),
            ('waiting','plan','install','now');
         PRAGMA query_only=ON;",
        )
        .unwrap();

    let before = read_stats(&db).unwrap();
    assert_eq!(
        before,
        DashboardStats {
            managed_game_count: 1,
            installed_mod_count: 2,
            total_mod_count: 5
        }
    );
    let conn = db.conn();
    assert_eq!(
        conn.query_row("SELECT installed_mod_count FROM games", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        99
    );
    assert_eq!(
        conn.query_row("SELECT status FROM installs WHERE id='restored'", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "uninstalled"
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM removed_mod_items", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        4
    );
    conn.pragma_update(None, "query_only", false).unwrap();
    drop(conn);

    install::reconcile(&db).unwrap();
    assert_eq!(read_stats(&db).unwrap(), before);
}
