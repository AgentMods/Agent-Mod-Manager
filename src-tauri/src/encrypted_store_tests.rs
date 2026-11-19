use super::*;

#[test]
fn authenticated_roundtrip_random_nonce_and_record_binding() {
    let key = [42; 32];
    let secret = "sk-test-not-a-real-credential";
    let first = encrypt(&key, "model.api_key", secret).unwrap();
    let second = encrypt(&key, "model.api_key", secret).unwrap();
    assert_ne!(first, second);
    assert!(!first.to_string().contains(secret));
    assert_eq!(
        decrypt(&key, "model.api_key", first.clone()).unwrap(),
        secret
    );
    assert!(decrypt(&[43; 32], "model.api_key", first.clone()).is_err());
    assert!(decrypt(&key, "sources.nexus", first.clone()).is_err());
    let mut corrupted = first;
    let bytes = corrupted["ciphertext"].as_str().unwrap().to_owned();
    corrupted["ciphertext"] = Value::String(format!(
        "{}{}",
        if &bytes[..2] == "00" { "01" } else { "00" },
        &bytes[2..]
    ));
    assert!(decrypt(&key, "model.api_key", corrupted).is_err());
}

#[test]
fn malformed_envelopes_and_oversized_plaintext_fail_without_echoing_secrets() {
    let key = [17; 32];
    assert!(encrypt(&key, "bad/key", "secret").is_err());
    assert!(encrypt(&key, "model.api_key", &"x".repeat(MAX_SECRET + 1)).is_err());
    for value in [
        serde_json::json!("sk-private"),
        serde_json::json!({"version":1,"algorithm":"AES-256-GCM","nonce":"bad","ciphertext":"sk-private"}),
    ] {
        let error = decrypt(&key, "model.api_key", value).unwrap_err();
        assert!(!error.to_string().contains("sk-private"));
    }
}

#[test]
fn runtime_env_is_generated_once_and_never_regenerated_for_existing_ciphertext() {
    let temp = tempfile::tempdir().unwrap();
    let env = temp.path().canonicalize().unwrap().join(".env");
    let ambient = std::env::var_os(KEY_VARIABLE);
    let first = master_key(&env, false, true).unwrap();
    assert_eq!(&*first, &*master_key(&env, true, true).unwrap());
    assert_eq!(fs::read_to_string(&env).unwrap().lines().count(), 1);
    let missing = temp.path().join("missing.env");
    assert!(master_key(&missing, true, true).is_err());
    assert!(!missing.exists());
    assert!(master_key(&missing, false, false).is_err());
    assert!(!missing.exists());
    assert_eq!(std::env::var_os(KEY_VARIABLE), ambient);
}

#[test]
fn invalid_duplicate_and_missing_env_keys_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().canonicalize().unwrap().join(".env");
    for text in [
        "OTHER=value\n".to_owned(),
        format!("{KEY_VARIABLE}=short\n"),
        format!(
            "{KEY_VARIABLE}={}\n{KEY_VARIABLE}={}\n",
            "a".repeat(64),
            "b".repeat(64)
        ),
    ] {
        if path.exists() {
            fs::write(&path, text).unwrap();
        } else {
            create_private(&path, text.as_bytes()).unwrap();
        }
        assert!(master_key(&path, true, false).is_err());
    }
}

#[test]
fn actual_tauri_store_persists_ciphertext_and_reopens_without_keychain() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let env = root.join(".env");
    let path = root.join(STORE_FILE);
    let app = tauri::test::mock_builder()
        .plugin(tauri_plugin_store::Builder::default().build())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let store = open_at(app.handle(), &path, &env, true).unwrap();
    let secret = "sk-store-test-not-a-real-key";
    store.write("model.api_key", secret).unwrap();
    let first = fs::read(&path).unwrap();
    assert!(!String::from_utf8_lossy(&first).contains(secret));
    assert_eq!(
        store.read("model.api_key").unwrap().as_deref(),
        Some(secret)
    );
    assert!(store.read("sources.missing").unwrap().is_none());
    store.write("model.api_key", secret).unwrap();
    assert_ne!(fs::read(&path).unwrap(), first);
    drop(store);
    let reopened = open_at(app.handle(), &path, &env, false).unwrap();
    assert_eq!(
        reopened.read("model.api_key").unwrap().as_deref(),
        Some(secret)
    );
    create_private(
        &root.join("wrong.env"),
        format!("{KEY_VARIABLE}={}\n", "ff".repeat(32)).as_bytes(),
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    assert!(open_at(app.handle(), &path, &root.join("wrong.env"), false).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn corrupt_store_is_not_silently_overwritten_by_plugin_load_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let env = root.join(".env");
    master_key(&env, false, true).unwrap();
    let path = root.join(STORE_FILE);
    create_private(&path, b"broken JSON").unwrap();
    let app = tauri::test::mock_builder()
        .plugin(tauri_plugin_store::Builder::default().build())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    assert!(open_at(app.handle(), &path, &env, false).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"broken JSON");
}

#[cfg(unix)]
#[test]
fn private_files_reject_symlinks_hardlinks_and_group_read_permissions() {
    use std::os::unix::{
        fs::{symlink, PermissionsExt},
        prelude::MetadataExt,
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let env = root.join(".env");
    master_key(&env, false, true).unwrap();
    assert_eq!(fs::metadata(&env).unwrap().mode() & 0o777, 0o600);
    let linked = root.join("linked.env");
    symlink(&env, &linked).unwrap();
    assert!(master_key(&linked, true, false).is_err());
    fs::set_permissions(&env, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(master_key(&env, true, false).is_err());
    fs::set_permissions(&env, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&env, root.join("hardlinked.env")).unwrap();
    assert!(master_key(&env, true, false).is_err());
}
