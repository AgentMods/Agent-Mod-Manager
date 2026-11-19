use agent_mod_manager_lib::{
    db::Db,
    encrypted_store,
    settings::{Settings, MODEL_API_KEY},
    storage::AppPaths,
    transaction::{FileEngine, FileOp},
};
use tauri::test::{mock_builder, mock_context, noop_assets};

fn app() -> tauri::App<tauri::test::MockRuntime> {
    mock_builder()
        .plugin(tauri_plugin_store::Builder::default().build())
        .build(mock_context(noop_assets()))
        .unwrap()
}

#[test]
fn real_store_and_settings_survive_new_app_instance_without_plaintext_on_disk() {
    let temp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(temp.path().join("manager")).unwrap();
    let db = Db::open(&paths.db_file()).unwrap();
    let instance = app();
    let settings = Settings::with_secret_store(
        db.clone(),
        encrypted_store::open(instance.handle(), &paths.root).unwrap(),
    );
    let secret = "sk-INTEGRATION-TEST-not-a-real-key";
    settings.write_secret(MODEL_API_KEY, secret).unwrap();
    settings
        .set("api_key_ref", serde_json::json!(MODEL_API_KEY))
        .unwrap();
    let path = paths.root.join("private/credentials.json");
    let first = std::fs::read(&path).unwrap();
    let env = paths.root.join("private/.env");
    let key_line = std::fs::read_to_string(&env).unwrap();
    let key = key_line.trim().split_once('=').unwrap().1;
    assert_eq!(key.len(), 64);
    assert!(!String::from_utf8_lossy(&first).contains(key));
    assert!(!String::from_utf8_lossy(&first).contains(secret));
    settings.write_secret(MODEL_API_KEY, secret).unwrap();
    assert_ne!(std::fs::read(&path).unwrap(), first);
    drop(settings);
    drop(instance);
    let instance = app();
    let settings = Settings::with_secret_store(
        db.clone(),
        encrypted_store::open(instance.handle(), &paths.root).unwrap(),
    );
    assert_eq!(
        settings.read_secret(MODEL_API_KEY).unwrap().as_deref(),
        Some(secret)
    );
    assert_eq!(
        settings.load().unwrap().api_key_ref.as_deref(),
        Some(MODEL_API_KEY)
    );
    let refs: String = db
        .conn()
        .query_row(
            "SELECT service FROM secrets_ref WHERE key=?1",
            [MODEL_API_KEY],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(refs, "tauri-store:aes-256-gcm:v1");
    assert!(!String::from_utf8_lossy(&std::fs::read(paths.db_file()).unwrap()).contains(secret));
    let engine = FileEngine::new(db, paths.clone());
    for target in [path, env] {
        assert!(agent_mod_manager_lib::filesystem::read_file(&engine, &target).is_err());
        assert!(engine
            .plan_file_ops(
                vec![FileOp::Copy {
                    src: target,
                    dst: paths.staging().join("leak.txt")
                }],
                "deny".into()
            )
            .is_err());
    }
}

#[test]
fn missing_key_for_existing_store_and_tampered_ciphertext_preserve_originals() {
    let temp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(temp.path().join("manager")).unwrap();
    let instance = app();
    let store = encrypted_store::open(instance.handle(), &paths.root).unwrap();
    store.write("model.api_key", "test-only").unwrap();
    drop(store);
    drop(instance);
    let env = paths.root.join("private/.env");
    let parked = paths.root.join("private/.env.parked");
    std::fs::rename(&env, &parked).unwrap();
    let path = paths.root.join("private/credentials.json");
    let before = std::fs::read(&path).unwrap();
    let instance = app();
    assert!(encrypted_store::open(instance.handle(), &paths.root).is_err());
    assert!(!env.exists());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    std::fs::rename(&parked, &env).unwrap();
    let mut data: serde_json::Value = serde_json::from_slice(&before).unwrap();
    data["model.api_key"]["ciphertext"] = serde_json::json!("00".repeat(32));
    let broken = serde_json::to_vec(&data).unwrap();
    std::fs::write(&path, &broken).unwrap();
    assert!(encrypted_store::open(instance.handle(), &paths.root).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), broken);
}

#[test]
fn legacy_reference_cannot_access_the_new_encrypted_backend() {
    let temp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(temp.path().join("manager")).unwrap();
    let db = Db::open(&paths.db_file()).unwrap();
    db.conn().execute("INSERT INTO secrets_ref(key,service,account,updated_at) VALUES(?1,'com.agent-mod-manager',?1,'legacy')", [MODEL_API_KEY]).unwrap();
    let instance = app();
    let settings = Settings::with_secret_store(
        db,
        encrypted_store::open(instance.handle(), &paths.root).unwrap(),
    );
    settings
        .set("api_key_ref", serde_json::json!(MODEL_API_KEY))
        .unwrap();
    assert_eq!(settings.read_secret(MODEL_API_KEY).unwrap(), None);
    assert_eq!(settings.load().unwrap().api_key_ref, None);
    settings
        .write_secret(MODEL_API_KEY, "replacement-test-key")
        .unwrap();
    assert_eq!(
        settings.read_secret(MODEL_API_KEY).unwrap().as_deref(),
        Some("replacement-test-key")
    );
    assert_eq!(
        settings.load().unwrap().api_key_ref.as_deref(),
        Some(MODEL_API_KEY)
    );
}
