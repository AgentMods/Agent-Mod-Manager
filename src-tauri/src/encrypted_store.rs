//! Rust-only Tauri Store. Values and change events contain authenticated
//! ciphertext, never plaintext. The runtime .env is parsed, not exported into
//! the process environment or compiled into the frontend.
use crate::{
    error::{AppError, AppResult},
    settings::{validate_secret_key, SecretStore},
};
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use ring::rand::{generate, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tauri::Runtime;
use tauri_plugin_store::{Store, StoreBuilder};
use zeroize::Zeroizing;

const KEY_VARIABLE: &str = "AMM_STORE_ENCRYPTION_KEY";
const MAX_SECRET: usize = 16 * 1024;
const MAX_ENTRIES: usize = 128;
const MAX_STORE: u64 = 5 * 1024 * 1024;
const STORE_FILE: &str = "credentials.json";

fn failure(message: &str) -> AppError {
    AppError::Other(message.into())
}
fn random<const N: usize>() -> AppResult<[u8; N]> {
    generate::<[u8; N]>(&SystemRandom::new())
        .map(|bytes| bytes.expose())
        .map_err(|_| failure("无法取得系统安全随机数"))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u8,
    algorithm: String,
    nonce: String,
    ciphertext: String,
}

fn encrypt(key: &[u8; 32], name: &str, value: &str) -> AppResult<Value> {
    validate_secret_key(name)?;
    if value.len() > MAX_SECRET {
        return Err(failure("凭据超过 16 KiB 上限"));
    }
    let nonce = random::<12>()?;
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| failure("加密密钥格式无效"))?;
    let aad = format!("amm:credentials:v1:{name}");
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: value.as_bytes(),
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| failure("凭据加密失败"))?;
    Ok(serde_json::to_value(Envelope {
        version: 1,
        algorithm: "AES-256-GCM".into(),
        nonce: hex::encode(nonce),
        ciphertext: hex::encode(ciphertext),
    })?)
}
fn decrypt(key: &[u8; 32], name: &str, value: Value) -> AppResult<String> {
    validate_secret_key(name)?;
    let envelope: Envelope =
        serde_json::from_value(value).map_err(|_| failure("凭据密文格式无效，未覆盖原文件"))?;
    if envelope.version != 1
        || envelope.algorithm != "AES-256-GCM"
        || envelope.nonce.len() != 24
        || envelope.ciphertext.len() < 32
        || envelope.ciphertext.len() > (MAX_SECRET + 16) * 2
    {
        return Err(failure("凭据密文版本或长度无效"));
    }
    let nonce = hex::decode(envelope.nonce).map_err(|_| failure("凭据 nonce 格式无效"))?;
    let ciphertext = hex::decode(envelope.ciphertext).map_err(|_| failure("凭据密文格式无效"))?;
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| failure("加密密钥格式无效"))?;
    let aad = format!("amm:credentials:v1:{name}");
    let bytes = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &ciphertext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| failure("凭据解密校验失败：.env 密钥不匹配或密文已损坏，未覆盖原文件"))?;
    String::from_utf8(bytes).map_err(|_| failure("凭据解密内容不是 UTF-8"))
}

pub(crate) fn private_directory(path: &Path) -> AppResult<()> {
    if !path.exists() {
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder.create(path)?;
    }
    if !fs::symlink_metadata(path)?.is_dir() || path.canonicalize()? != path {
        return Err(failure("凭据目录必须是未重定向的普通目录"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if fs::symlink_metadata(path)?.mode() & 0o077 != 0 {
            return Err(failure("凭据目录必须仅当前用户可访问；请设置权限为 700"));
        }
    }
    Ok(())
}
fn private_file(path: &Path, limit: u64) -> AppResult<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() || meta.len() > limit || path.canonicalize()? != path {
        return Err(failure("凭据文件必须是大小受限且未重定向的普通文件"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o077 != 0 || meta.nlink() != 1 {
            return Err(failure(
                "凭据文件必须仅当前用户可读写且不能是硬链接；请设置权限为 600",
            ));
        }
    }
    Ok(())
}
fn create_private(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn master_key(
    env_path: &Path,
    store_exists: bool,
    generate: bool,
) -> AppResult<Zeroizing<[u8; 32]>> {
    if !env_path.exists() {
        if store_exists || !generate {
            return Err(failure(
                "凭据 .env 文件不存在；不会重新生成密钥或读取系统钥匙串",
            ));
        }
        let key = Zeroizing::new(random::<32>()?);
        let content = Zeroizing::new(format!("{KEY_VARIABLE}={}\n", hex::encode(key.as_slice())));
        create_private(env_path, content.as_bytes())?;
    }
    private_file(env_path, 16 * 1024)?;
    let mut bytes = Zeroizing::new(Vec::new());
    fs::File::open(env_path)?
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 {
        return Err(failure("凭据 .env 文件过大"));
    }
    let mut found = None;
    for entry in dotenvy::from_read_iter(bytes.as_slice()) {
        let (name, value) = entry.map_err(|_| failure("凭据 .env 格式无效"))?;
        let value = Zeroizing::new(value);
        if name == KEY_VARIABLE {
            if found.is_some() {
                return Err(failure(".env 中的加密密钥不能重复定义"));
            }
            let mut key = Zeroizing::new([0; 32]);
            if value.len() != 64 || hex::decode_to_slice(value.as_bytes(), &mut *key).is_err() {
                return Err(failure(
                    "AMM_STORE_ENCRYPTION_KEY 必须是 64 位十六进制随机密钥",
                ));
            }
            found = Some(key);
        }
    }
    found.ok_or_else(|| failure(".env 缺少 AMM_STORE_ENCRYPTION_KEY"))
}

struct EncryptedStore<R: Runtime> {
    store: Arc<Store<R>>,
    key: Zeroizing<[u8; 32]>,
    path: PathBuf,
    writes: Mutex<()>,
}
impl<R: Runtime> SecretStore for EncryptedStore<R> {
    fn read(&self, name: &str) -> AppResult<Option<String>> {
        validate_secret_key(name)?;
        let _lock = self
            .writes
            .lock()
            .map_err(|_| failure("凭据存储锁不可用"))?;
        private_file(&self.path, MAX_STORE)?;
        self.store
            .get(name)
            .map(|value| decrypt(&self.key, name, value))
            .transpose()
    }
    fn write(&self, name: &str, value: &str) -> AppResult<()> {
        validate_secret_key(name)?;
        let _lock = self
            .writes
            .lock()
            .map_err(|_| failure("凭据存储锁不可用"))?;
        private_file(&self.path, MAX_STORE)?;
        let previous = self.store.get(name);
        if previous.is_none() && self.store.length() >= MAX_ENTRIES {
            return Err(failure("凭据存储超过 128 条上限"));
        }
        self.store.set(name, encrypt(&self.key, name, value)?);
        if self.store.save().is_err() {
            match previous {
                Some(value) => self.store.set(name, value),
                None => {
                    self.store.delete(name);
                }
            }
            return Err(failure("加密凭据保存失败，未登记凭据引用"));
        }
        // Windows FlushFileBuffers requires write access; a read-only handle fails with os error 5.
        fs::OpenOptions::new()
            .write(true)
            .open(&self.path)?
            .sync_all()?;
        Ok(())
    }
}

/// Only a runtime absolute path may override the default .env. No include_str!,
/// VITE_ variable, frontend command or third-party process receives the key.
pub fn open<R: Runtime>(
    app: &tauri::AppHandle<R>,
    data_root: &Path,
) -> AppResult<Arc<dyn SecretStore>> {
    let private = data_root.join("private");
    private_directory(&private)?;
    let configured = std::env::var_os("AMM_STORE_ENV_FILE").map(PathBuf::from);
    let env_path = configured.clone().unwrap_or_else(|| private.join(".env"));
    if !env_path.is_absolute() {
        return Err(failure("AMM_STORE_ENV_FILE 必须是绝对路径"));
    }
    if !env_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == ".env" || name.starts_with(".env."))
    {
        return Err(failure("AMM_STORE_ENV_FILE 必须指向 .env 或 .env.* 文件"));
    }
    open_at(
        app,
        &private.join(STORE_FILE),
        &env_path,
        configured.is_none(),
    )
}
fn open_at<R: Runtime>(
    app: &tauri::AppHandle<R>,
    path: &Path,
    env_path: &Path,
    generate: bool,
) -> AppResult<Arc<dyn SecretStore>> {
    let exists = match fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => return Err(failure("无法检查凭据 Store，不会生成新密钥")),
    };
    let key = master_key(env_path, exists, generate)?;
    if !exists {
        create_private(path, b"{}")?;
    }
    private_file(path, MAX_STORE)?;
    let bytes = fs::read(path)?;
    let records: HashMap<String, Value> =
        serde_json::from_slice(&bytes).map_err(|_| failure("凭据 Store 损坏，拒绝用空文件覆盖"))?;
    if records.len() > MAX_ENTRIES {
        return Err(failure("凭据存储条目过多"));
    }
    for (name, value) in records {
        let _plaintext = Zeroizing::new(decrypt(&key, &name, value)?);
    }
    let store = StoreBuilder::new(app, path)
        .disable_auto_save()
        .build()
        .map_err(|_| failure("无法打开加密凭据 Store"))?;
    // StoreBuilder ignores initial load errors, so explicitly check reload.
    store
        .reload_ignore_defaults()
        .map_err(|_| failure("无法读取加密凭据 Store"))?;
    Ok(Arc::new(EncryptedStore {
        store,
        key,
        path: path.into(),
        writes: Mutex::new(()),
    }))
}

#[cfg(test)]
#[path = "encrypted_store_tests.rs"]
mod tests;
