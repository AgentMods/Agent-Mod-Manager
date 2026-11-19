//! Public configuration and encrypted-Store references. Secret values never enter SQLite.

use crate::{
    db::Db,
    error::{AppError, AppResult},
    util::now,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

pub const SECRET_STORE_SERVICE: &str = "tauri-store:aes-256-gcm:v1";
pub const MODEL_API_KEY: &str = "model.api_key";
const PUBLIC_KEYS: &[&str] = &[
    "base_url",
    "model",
    "context_budget",
    "permission_mode",
    "api_key_ref",
];

/// This DTO contains configuration and an optional reference, never a credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppSettings {
    pub base_url: String,
    pub model: String,
    /// Maximum input-context tokens retained by the Agent.
    pub context_budget: usize,
    pub permission_mode: String,
    pub api_key_ref: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".into(),
            model: String::new(),
            context_budget: 32_768,
            permission_mode: "request_approval".into(),
            api_key_ref: None,
        }
    }
}

/// Deliberately has no Debug/Serialize bound: implementations may own secrets.
pub trait SecretStore: Send + Sync {
    fn read(&self, key: &str) -> AppResult<Option<String>>;
    fn write(&self, key: &str, value: &str) -> AppResult<()>;
}

/// Clone shares the secret provider; credentials are neither cached nor logged.
#[derive(Clone)]
pub struct Settings {
    pub db: Db,
    secrets: Arc<dyn SecretStore>,
}

impl Settings {
    pub fn with_secret_store(db: Db, secrets: Arc<dyn SecretStore>) -> Self {
        Self { db, secrets }
    }

    pub fn load(&self) -> AppResult<AppSettings> {
        let conn = self.db.conn();
        let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
        let mut rows = stmt.query([])?;
        let mut values = serde_json::to_value(AppSettings::default())?;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            if PUBLIC_KEYS.contains(&key.as_str()) {
                let encoded: String = row.get(1)?;
                values[&key] = serde_json::from_str(&encoded)?;
            }
        }
        let mut settings: AppSettings = serde_json::from_value(values)?;
        validate_settings(&settings)?;
        if let Some(key) = &settings.api_key_ref {
            let encrypted: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM secrets_ref WHERE key=?1 AND service=?2 AND account=?1)",
                params![key, SECRET_STORE_SERVICE], |row| row.get(0),
            )?;
            if !encrypted {
                settings.api_key_ref = None;
            }
        }
        Ok(settings)
    }

    /// Validate the whole DTO before atomically saving any of its public fields.
    pub fn save(&self, settings: &AppSettings) -> AppResult<()> {
        validate_settings(settings)?;
        let value = serde_json::to_value(settings)?;
        let mut conn = self.db.conn();
        let tx = conn.transaction()?;
        for key in PUBLIC_KEYS {
            tx.execute(
                "INSERT INTO settings(key,value) VALUES(?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, serde_json::to_string(&value[key])?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Only public fields and conversation-scoped permission keys are exposed.
    pub fn get(&self, key: &str) -> AppResult<Option<Value>> {
        validate_setting_key(key)?;
        let encoded: Option<String> = self
            .db
            .conn()
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .optional()?;
        encoded
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(Into::into)
    }

    pub fn set(&self, key: &str, value: Value) -> AppResult<()> {
        validate_setting_key(key)?;
        if is_conversation_permission_key(key) {
            let mode = value
                .as_str()
                .ok_or_else(|| AppError::Invalid("permission_mode 必须为字符串".into()))?;
            validate_permission_mode(mode)?;
            let id = &key["conversation.".len()..key.len() - ".permission_mode".len()];
            let mut conn = self.db.conn();
            let tx = conn.transaction()?;
            if !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=?1)",
                [id],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(AppError::NotFound("会话".into()));
            }
            tx.execute(
                "INSERT INTO settings(key,value) VALUES(?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, serde_json::to_string(&value)?],
            )?;
            tx.commit()?;
            return Ok(());
        }
        // Validate in isolation: a single-key update never rewrites other fields.
        let mut candidate = serde_json::to_value(AppSettings::default())?;
        candidate[key] = value.clone();
        validate_settings(&serde_json::from_value(candidate)?)?;
        self.db.conn().execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, serde_json::to_string(&value)?],
        )?;
        Ok(())
    }

    pub fn read_secret(&self, key: &str) -> AppResult<Option<String>> {
        validate_secret_key(key)?;
        let registered = self.db.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM secrets_ref WHERE key=?1 AND service=?2 AND account=?1)",
            params![key, SECRET_STORE_SERVICE],
            |row| row.get::<_, bool>(0),
        )?;
        if registered {
            self.secrets.read(key)
        } else {
            Ok(None)
        }
    }

    /// First persist to the secure backend. A failed backend write creates no ref.
    /// Retrying after a SQLite failure safely overwrites the same encrypted record.
    pub fn write_secret(&self, key: &str, value: &str) -> AppResult<()> {
        validate_secret_key(key)?;
        self.secrets.write(key, value)?;
        self.db.conn().execute(
            "INSERT INTO secrets_ref(key,service,account,updated_at) VALUES(?1,?2,?1,?3)
             ON CONFLICT(key) DO UPDATE SET service=excluded.service,account=excluded.account,
             updated_at=excluded.updated_at",
            params![key, SECRET_STORE_SERVICE, now()],
        )?;
        Ok(())
    }
}

pub(crate) fn conversation_permission_key(id: &str) -> String {
    format!("conversation.{id}.permission_mode")
}

pub(crate) fn validate_permission_mode(mode: &str) -> AppResult<()> {
    if !matches!(mode, "request_approval" | "auto_approve" | "full_trust") {
        return Err(AppError::Invalid("无效的 permission_mode".into()));
    }
    Ok(())
}

fn validate_settings(settings: &AppSettings) -> AppResult<()> {
    let url = url::Url::parse(&settings.base_url)
        .map_err(|_| AppError::Invalid("无效的模型 base_url".into()))?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AppError::Invalid(
            "base_url 仅支持无认证信息的 HTTP(S) 地址".into(),
        ));
    }
    if settings.model.len() > 256 || settings.model.chars().any(char::is_control) {
        return Err(AppError::Invalid("无效的模型名称".into()));
    }
    if settings.context_budget == 0 || settings.context_budget > 16_777_216 {
        return Err(AppError::Invalid("context_budget 超出范围".into()));
    }
    validate_permission_mode(&settings.permission_mode)?;
    if let Some(key) = &settings.api_key_ref {
        validate_secret_key(key)?;
    }
    Ok(())
}

pub(crate) fn validate_secret_key(key: &str) -> AppResult<()> {
    if key.is_empty()
        || key.len() > 128
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(AppError::Invalid("无效的密钥引用".into()));
    }
    Ok(())
}

fn is_conversation_permission_key(key: &str) -> bool {
    key.strip_prefix("conversation.")
        .and_then(|key| key.strip_suffix(".permission_mode"))
        .is_some_and(|id| {
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}

fn validate_setting_key(key: &str) -> AppResult<()> {
    if PUBLIC_KEYS.contains(&key) || is_conversation_permission_key(key) {
        Ok(())
    } else {
        Err(AppError::Invalid("不支持的设置键".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{collections::HashMap, sync::Mutex};

    #[derive(Default)]
    struct MockSecrets(Mutex<HashMap<String, String>>);
    impl SecretStore for MockSecrets {
        fn read(&self, key: &str) -> AppResult<Option<String>> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        fn write(&self, key: &str, value: &str) -> AppResult<()> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
    }

    #[test]
    fn secret_values_stay_in_provider_and_out_of_database_and_dto() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.sqlite");
        let db = Db::open(&path).unwrap();
        let store = Arc::new(MockSecrets::default());
        let settings = Settings::with_secret_store(db.clone(), store.clone());
        assert_eq!(settings.load().unwrap(), AppSettings::default());
        assert_eq!(settings.read_secret(MODEL_API_KEY).unwrap(), None);
        let secret = "sk-MOCK-credential-should-never-be-persisted";
        settings.write_secret(MODEL_API_KEY, secret).unwrap();
        settings.set("api_key_ref", json!(MODEL_API_KEY)).unwrap();
        assert_eq!(
            settings.read_secret(MODEL_API_KEY).unwrap().as_deref(),
            Some(secret)
        );
        let dto = settings.load().unwrap();
        assert!(!format!("{dto:?}").contains(secret));
        assert!(!serde_json::to_string(&dto).unwrap().contains(secret));
        assert!(settings.set("api_key", json!(secret)).is_err());
        assert!(settings.set("secrets_ref", json!(secret)).is_err());
        assert!(settings.get("api_key").is_err());
        assert!(serde_json::from_value::<AppSettings>(json!({"api_key":secret})).is_err());
        let conn = db.conn();
        let dump: String = conn.query_row(
            "SELECT COALESCE((SELECT group_concat(key || value) FROM settings),'') ||
             COALESCE((SELECT group_concat(key || service || account || updated_at) FROM secrets_ref),'')",
            [], |r| r.get(0),
        ).unwrap();
        assert!(!dump.contains(secret));
        assert!(dump.contains(MODEL_API_KEY));
        drop(conn);
        drop(settings);
        drop(db);
        assert!(!std::fs::read(&path)
            .unwrap()
            .windows(secret.len())
            .any(|w| w == secret.as_bytes()));
        let reopened = Settings::with_secret_store(Db::open(&path).unwrap(), store);
        assert_eq!(
            reopened.load().unwrap().api_key_ref.as_deref(),
            Some(MODEL_API_KEY)
        );
        assert_eq!(
            reopened.read_secret(MODEL_API_KEY).unwrap().as_deref(),
            Some(secret)
        );
    }

    #[test]
    fn invalid_settings_are_rejected_atomically_and_without_echoing_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings::with_secret_store(
            Db::open(&dir.path().join("db")).unwrap(),
            Arc::new(MockSecrets::default()),
        );
        let original = AppSettings {
            model: "test-model".into(),
            ..Default::default()
        };
        settings.save(&original).unwrap();
        let mut invalid = original.clone();
        invalid.model = "changed-model".into();
        invalid.context_budget = 0;
        assert!(settings.save(&invalid).is_err());
        assert_eq!(settings.load().unwrap(), original);
        for url in [
            "https://user:sk-private@example.com/v1",
            "https://example.com?api_key=sk-private",
            "file:///tmp/model",
        ] {
            let error = settings.set("base_url", json!(url)).unwrap_err();
            assert!(!error.to_string().contains("sk-private"));
        }
        assert!(settings.set("context_budget", json!(-1)).is_err());
        assert!(settings.set("permission_mode", json!(" ")).is_err());
        assert!(settings.set("permission_mode", json!("confirm")).is_err());
        assert!(settings
            .set(
                "conversation.missing.permission_mode",
                json!("request_approval")
            )
            .is_err());
        assert_eq!(settings.load().unwrap(), original);
    }

    #[test]
    fn a_failed_secret_write_does_not_register_a_reference() {
        struct Unavailable;
        impl SecretStore for Unavailable {
            fn read(&self, _: &str) -> AppResult<Option<String>> {
                Err(AppError::Other("offline".into()))
            }
            fn write(&self, _: &str, _: &str) -> AppResult<()> {
                Err(AppError::Other("offline".into()))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("db")).unwrap();
        let settings = Settings::with_secret_store(db.clone(), Arc::new(Unavailable));
        assert!(settings.write_secret(MODEL_API_KEY, "a secret").is_err());
        assert_eq!(
            db.conn()
                .query_row("SELECT count(*) FROM secrets_ref", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn legacy_keychain_references_are_never_read_or_claimed_as_configured() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("db")).unwrap();
        struct RejectReads;
        impl SecretStore for RejectReads {
            fn read(&self, _: &str) -> AppResult<Option<String>> {
                panic!("legacy reference must not access any secret backend");
            }
            fn write(&self, _: &str, _: &str) -> AppResult<()> {
                Ok(())
            }
        }
        let settings = Settings::with_secret_store(db.clone(), Arc::new(RejectReads));
        db.conn().execute("INSERT INTO secrets_ref(key,service,account,updated_at) VALUES(?1,'com.agent-mod-manager',?1,'legacy')", [MODEL_API_KEY]).unwrap();
        settings.set("api_key_ref", json!(MODEL_API_KEY)).unwrap();
        assert_eq!(settings.load().unwrap().api_key_ref, None);
        assert_eq!(settings.read_secret(MODEL_API_KEY).unwrap(), None);
    }

    #[test]
    fn public_settings_and_conversation_overrides_share_the_exact_permission_modes() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("db")).unwrap();
        let settings = Settings::with_secret_store(db.clone(), Arc::new(MockSecrets::default()));
        let history = crate::history::History::new(db);
        let conversation = history.create_conversation("Permissions", None).unwrap();
        let key = conversation_permission_key(&conversation.id);
        assert_eq!(settings.load().unwrap().permission_mode, "request_approval");
        for mode in ["request_approval", "auto_approve", "full_trust"] {
            settings.set("permission_mode", json!(mode)).unwrap();
            settings.set(&key, json!(mode)).unwrap();
            assert_eq!(settings.load().unwrap().permission_mode, mode);
            assert_eq!(
                history
                    .get_conversation(&conversation.id)
                    .unwrap()
                    .conversation
                    .permission_mode
                    .as_deref(),
                Some(mode)
            );
        }
        assert!(settings.set(&key, json!("confirm")).is_err());
        settings.save(&AppSettings::default()).unwrap();
        assert_eq!(settings.get(&key).unwrap(), Some(json!("full_trust")));
        history.set_permission_mode(&conversation.id, None).unwrap();
        assert_eq!(settings.get(&key).unwrap(), None);
    }
}
