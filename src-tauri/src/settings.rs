//! Public configuration and encrypted-Store references. Secret values never enter SQLite.

use crate::{
    db::Db,
    error::{AppError, AppResult},
    util::now,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashSet},
    sync::Arc,
};

pub const SECRET_STORE_SERVICE: &str = "tauri-store:aes-256-gcm:v1";
pub const MODEL_API_KEY: &str = "model.api_key";
const PUBLIC_KEYS: &[&str] = &[
    "base_url",
    "model",
    "models",
    "permission_mode",
    "api_key_ref",
    "onboarding_completed",
];

const COMPOSER_PREFERENCES_KEY: &str = "composer_preferences";

const MOD_ORGANIZATION_LOCALE_KEY: &str = "mod_organization_locale";
pub(crate) const MOD_ORGANIZATION_LANGUAGES: &[(&str, &str)] = &[
    ("zh-CN", "简体中文"),
    ("zh-TW", "繁體中文"),
    ("en", "English"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("pt-BR", "Português (Brasil)"),
    ("it", "Italiano"),
    ("ru", "Русский"),
    ("pl", "Polski"),
    ("uk", "Українська"),
    ("tr", "Türkçe"),
];

fn is_mod_organization_override(locale: &str) -> bool {
    MOD_ORGANIZATION_LANGUAGES
        .iter()
        .any(|(code, _)| *code == locale)
}

pub(crate) fn mod_organization_locale_override(
    conn: &rusqlite::Connection,
) -> AppResult<Option<String>> {
    let encoded: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key=?1",
            [MOD_ORGANIZATION_LOCALE_KEY],
            |row| row.get(0),
        )
        .optional()?;
    Ok(encoded
        .and_then(|value| serde_json::from_str::<String>(&value).ok())
        .filter(|value| is_mod_organization_override(value)))
}

pub(crate) fn set_mod_organization_locale_override(db: &Db, locale: Option<&str>) -> AppResult<()> {
    if let Some(locale) = locale {
        if !is_mod_organization_override(locale) {
            return Err(AppError::Invalid("不支持的 Mod 整理目标语言".into()));
        }
        db.conn().execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![MOD_ORGANIZATION_LOCALE_KEY, serde_json::to_string(locale)?],
        )?;
    } else {
        db.conn().execute(
            "DELETE FROM settings WHERE key=?1",
            [MOD_ORGANIZATION_LOCALE_KEY],
        )?;
    }
    Ok(())
}

fn system_mod_organization_locale(locale: Option<String>) -> String {
    locale
        .filter(|locale| {
            let mut subtags = locale.split('-');
            locale.len() <= 64
                && subtags.next().is_some_and(|language| {
                    (2..=8).contains(&language.len())
                        && language.bytes().all(|byte| byte.is_ascii_alphabetic())
                        && !matches!(language.to_ascii_lowercase().as_str(), "posix" | "und")
                })
                && subtags.all(|tag| {
                    (1..=8).contains(&tag.len())
                        && tag.bytes().all(|byte| byte.is_ascii_alphanumeric())
                })
        })
        .unwrap_or_else(|| "zh-CN".into())
}

pub(crate) fn system_mod_locale() -> String {
    system_mod_organization_locale(sys_locale::get_locale())
}

/// Effective target for new Mod organization runs, independent of model and UI settings.
pub fn mod_organization_locale(db: &Db) -> String {
    effective_mod_locale(&db.conn())
}

pub(crate) fn effective_mod_locale(conn: &rusqlite::Connection) -> String {
    mod_organization_locale_override(conn)
        .ok()
        .flatten()
        .unwrap_or_else(system_mod_locale)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Ultra,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "StoredAgentModelSettings")]
pub struct AgentModelSettings {
    pub display_name: String,
    pub id: String,
    pub reasoning_levels: Vec<ReasoningEffort>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAgentModelSettings {
    display_name: Option<String>,
    id: String,
    // Older rows may contain any JSON value; never use it as model metadata.
    #[serde(rename = "context_budget")]
    _legacy_context_budget: Option<serde::de::IgnoredAny>,
    reasoning_levels: Option<Vec<ReasoningEffort>>,
    reasoning_effort: Option<ReasoningEffort>,
}

impl From<StoredAgentModelSettings> for AgentModelSettings {
    fn from(stored: StoredAgentModelSettings) -> Self {
        Self {
            display_name: stored
                .display_name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| stored.id.clone()),
            id: stored.id,
            reasoning_levels: stored
                .reasoning_levels
                .unwrap_or_else(|| stored.reasoning_effort.into_iter().collect()),
        }
    }
}

/// The model, reasoning level and permission policy used by the most recent turn.
/// Kept apart from `AppSettings` so saving the settings page cannot overwrite it.
/// `None` means the composer falls back to the saved default / conversation policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ComposerPreferences {
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub permission_mode: Option<String>,
}

/// This DTO contains configuration and an optional reference, never a credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppSettings {
    pub base_url: String,
    pub model: String,
    pub models: Vec<AgentModelSettings>,
    pub permission_mode: String,
    pub api_key_ref: Option<String>,
    pub onboarding_completed: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            base_url: "https://one.gloscai.com/v1".into(),
            model: String::new(),
            models: Vec::new(),
            permission_mode: "request_approval".into(),
            api_key_ref: None,
            onboarding_completed: false,
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

    /// A damaged or outdated record falls back to defaults instead of blocking the composer.
    pub fn load_composer_preferences(&self) -> AppResult<ComposerPreferences> {
        let encoded: Option<String> = self
            .db
            .conn()
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                [COMPOSER_PREFERENCES_KEY],
                |row| row.get(0),
            )
            .optional()?;
        let mut preferences: ComposerPreferences = encoded
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default();
        if preferences
            .model
            .as_deref()
            .is_some_and(|model| !valid_model_id(model))
        {
            preferences.model = None;
        }
        if preferences
            .permission_mode
            .as_deref()
            .is_some_and(|mode| validate_permission_mode(mode).is_err())
        {
            preferences.permission_mode = None;
        }
        Ok(preferences)
    }

    pub fn save_composer_preferences(&self, preferences: &ComposerPreferences) -> AppResult<()> {
        if preferences
            .model
            .as_deref()
            .is_some_and(|model| !valid_model_id(model))
        {
            return Err(AppError::Invalid("无效的模型名称".into()));
        }
        if let Some(mode) = &preferences.permission_mode {
            validate_permission_mode(mode)?;
        }
        self.db.conn().execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![
                COMPOSER_PREFERENCES_KEY,
                serde_json::to_string(preferences)?
            ],
        )?;
        Ok(())
    }

    pub fn onboarding_state(&self) -> AppResult<(bool, bool, bool)> {
        let (completed, started, has_games): (Option<String>, bool, bool) =
            self.db.conn().query_row(
                "SELECT (SELECT value FROM settings WHERE key='onboarding_completed'),
                    EXISTS(SELECT 1 FROM settings WHERE key='onboarding_in_progress'),
                    EXISTS(SELECT 1 FROM games)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        let completed = completed
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or(false);
        Ok((completed, !completed && !has_games, !completed && started))
    }

    /// Record that a fresh profile can resume even after registering a game.
    pub fn start_onboarding(&self) -> AppResult<()> {
        self.db.conn().execute(
            "INSERT OR IGNORE INTO settings(key,value) VALUES('onboarding_in_progress','true')",
            [],
        )?;
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
        candidate[key] = value;
        let candidate = serde_json::from_value(candidate)?;
        validate_settings(&candidate)?;
        let value = serde_json::to_value(candidate)?;
        self.db.conn().execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, serde_json::to_string(&value[key])?],
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

pub(crate) fn parse_model_base_url(base_url: &str) -> AppResult<url::Url> {
    let url =
        url::Url::parse(base_url).map_err(|_| AppError::Invalid("无效的模型 base_url".into()))?;
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
    Ok(url)
}

pub(crate) fn valid_model_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && id.trim() == id && !id.chars().any(char::is_control)
}

fn validate_settings(settings: &AppSettings) -> AppResult<()> {
    parse_model_base_url(&settings.base_url)?;
    if settings.model.len() > 256 || settings.model.chars().any(char::is_control) {
        return Err(AppError::Invalid("无效的模型名称".into()));
    }
    let mut ids = HashSet::new();
    if settings.models.iter().any(|model| {
        !valid_model_id(&model.display_name) || !valid_model_id(&model.id) || !ids.insert(&model.id)
    }) {
        return Err(AppError::Invalid("模型列表含有重复 ID 或无效配置".into()));
    }
    for model in &settings.models {
        let mut levels = BTreeSet::new();
        if model
            .reasoning_levels
            .iter()
            .any(|level| !levels.insert(level))
        {
            return Err(AppError::Invalid("模型配置含有重复的思考等级".into()));
        }
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
    #[test]
    fn mod_organization_locale_is_independent_of_model_settings() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("db")).unwrap();
        let settings = Settings::with_secret_store(db.clone(), Arc::new(MockSecrets::default()));
        assert_eq!(mod_organization_locale_override(&db.conn()).unwrap(), None);
        let model_settings = AppSettings {
            model: "test-model".into(),
            ..Default::default()
        };
        settings.save(&model_settings).unwrap();
        set_mod_organization_locale_override(&db, Some("ja")).unwrap();
        assert_eq!(settings.load().unwrap(), model_settings);
        settings.save(&AppSettings::default()).unwrap();
        assert_eq!(mod_organization_locale(&db), "ja");
        for invalid in ["", "auto", "JA", " en", "xy", "en\nignore instructions"] {
            assert!(set_mod_organization_locale_override(&db, Some(invalid)).is_err());
        }
        assert_eq!(mod_organization_locale(&db), "ja");
        set_mod_organization_locale_override(&db, None).unwrap();
        assert_eq!(mod_organization_locale_override(&db.conn()).unwrap(), None);
        assert_eq!(
            mod_organization_locale(&db),
            system_mod_organization_locale(sys_locale::get_locale())
        );
    }

    #[test]
    fn mod_organization_locale_uses_system_tag_or_chinese_fallback() {
        for tag in ["en-US", "zh-Hant-TW", "ja", "es-419"] {
            assert_eq!(system_mod_organization_locale(Some(tag.into())), tag);
        }
        for tag in [
            None,
            Some("C"),
            Some("POSIX"),
            Some("und"),
            Some("en\nUS"),
            Some("en--US"),
        ] {
            assert_eq!(
                system_mod_organization_locale(tag.map(str::to_owned)),
                "zh-CN"
            );
        }
    }

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
    fn legacy_model_reasoning_effort_loads_as_an_enabled_level() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("settings.sqlite")).unwrap();
        let settings = Settings::with_secret_store(db, Arc::new(MockSecrets::default()));
        let legacy_models = json!([
            {"id":"old", "context_budget":32768, "reasoning_effort":"high"},
            {"id":"cleared", "display_name":"Custom label", "context_budget":{"invalid":true}, "reasoning_effort":"low", "reasoning_levels":[]},
            {"id":"no-budget", "reasoning_levels":[]}
        ]);
        settings.db.conn().execute(
            "INSERT INTO settings(key,value) VALUES ('models',?1), ('context_budget','bad JSON')",
            [legacy_models.to_string()],
        ).unwrap();
        let loaded = settings.load().unwrap();
        assert_eq!(
            loaded.models[0].reasoning_levels,
            vec![ReasoningEffort::High]
        );
        assert_eq!(loaded.models[0].display_name, "old");
        assert_eq!(loaded.models[1].display_name, "Custom label");
        assert!(loaded.models[1].reasoning_levels.is_empty());
        assert_eq!(loaded.models[2].id, "no-budget");
        assert!(serde_json::to_value(&loaded)
            .unwrap()
            .get("context_budget")
            .is_none());
        settings.save(&loaded).unwrap();
        let persisted = settings.get("models").unwrap().unwrap();
        assert_eq!(persisted[0]["reasoning_levels"], json!(["high"]));
        assert!(persisted[0].get("reasoning_effort").is_none());
        assert_eq!(persisted[1]["reasoning_levels"], json!([]));
        assert_eq!(persisted[1]["display_name"], "Custom label");
        assert!(persisted
            .as_array()
            .unwrap()
            .iter()
            .all(|model| model.get("context_budget").is_none()));
        assert!(settings.get("context_budget").is_err());
    }

    #[test]
    fn model_settings_persist_all_rows_in_order_above_old_quota() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.sqlite");
        let open = || {
            Settings::with_secret_store(Db::open(&path).unwrap(), Arc::new(MockSecrets::default()))
        };
        let models: Vec<_> = (0..100)
            .map(|index| AgentModelSettings {
                display_name: format!("模型 {index}"),
                id: format!("provider/model-{index}"),
                reasoning_levels: vec![ReasoningEffort::Medium],
            })
            .collect();
        let config = AppSettings {
            model: models[99].id.clone(),
            models: models.clone(),
            ..AppSettings::default()
        };
        open().save(&config).unwrap();
        let loaded = open().load().unwrap();
        assert_eq!(loaded.models, models);
        assert_eq!(loaded.model, config.model);
        let mut invalid = config;
        invalid.models[99].id = models[0].id.clone();
        assert!(open().save(&invalid).is_err());
        let reversed: Vec<_> = models.into_iter().rev().collect();
        open().set("models", json!(reversed)).unwrap();
        assert_eq!(open().load().unwrap().models, reversed);
    }

    #[test]
    fn composer_preferences_persist_across_reopen_and_ignore_damaged_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.sqlite");
        let open = || {
            Settings::with_secret_store(Db::open(&path).unwrap(), Arc::new(MockSecrets::default()))
        };
        assert_eq!(
            open().load_composer_preferences().unwrap(),
            ComposerPreferences::default()
        );
        let saved = ComposerPreferences {
            model: Some("fixture-thinking".into()),
            reasoning_effort: Some(ReasoningEffort::Ultra),
            permission_mode: Some("full_trust".into()),
        };
        open().save_composer_preferences(&saved).unwrap();
        // A new Db/Settings instance stands in for an application restart.
        let reopened = open();
        assert_eq!(reopened.load_composer_preferences().unwrap(), saved);
        // Saving the settings page must not touch the remembered turn choices.
        reopened.save(&AppSettings::default()).unwrap();
        assert_eq!(reopened.load_composer_preferences().unwrap(), saved);

        assert!(reopened
            .save_composer_preferences(&ComposerPreferences {
                permission_mode: Some("confirm".into()),
                ..ComposerPreferences::default()
            })
            .is_err());
        for damaged in [
            "not json",
            r#"{"reasoning_effort":"bogus"}"#,
            r#"{"unknown":1}"#,
        ] {
            reopened
                .db
                .conn()
                .execute(
                    "UPDATE settings SET value=?1 WHERE key=?2",
                    params![damaged, COMPOSER_PREFERENCES_KEY],
                )
                .unwrap();
            assert_eq!(
                reopened.load_composer_preferences().unwrap(),
                ComposerPreferences::default()
            );
        }
        reopened
            .db
            .conn()
            .execute(
                "UPDATE settings SET value=?1 WHERE key=?2",
                params![
                    r#"{"model":"kept","permission_mode":"confirm","reasoning_effort":"low"}"#,
                    COMPOSER_PREFERENCES_KEY
                ],
            )
            .unwrap();
        let partial = reopened.load_composer_preferences().unwrap();
        assert_eq!(partial.model.as_deref(), Some("kept"));
        assert_eq!(partial.reasoning_effort, Some(ReasoningEffort::Low));
        assert_eq!(partial.permission_mode, None);
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
        invalid.permission_mode = "invalid".into();
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

    #[test]
    fn onboarding_is_false_by_default_and_round_trips_through_public_settings() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings::with_secret_store(
            Db::open(&dir.path().join("db")).unwrap(),
            Arc::new(MockSecrets::default()),
        );
        assert!(!settings.load().unwrap().onboarding_completed);
        settings.set("onboarding_completed", json!(true)).unwrap();
        assert!(settings.load().unwrap().onboarding_completed);
    }

    #[test]
    fn onboarding_state_skips_existing_users_and_resumes_started_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(&temp.path().join("db")).unwrap();
        let settings = Settings::with_secret_store(db.clone(), Arc::new(MockSecrets::default()));
        assert_eq!(settings.onboarding_state().unwrap(), (false, true, false));

        settings.start_onboarding().unwrap();
        settings.start_onboarding().unwrap();
        let game = temp.path().join("game");
        std::fs::create_dir(&game).unwrap();
        crate::games::register_game(&db, &game, None).unwrap();
        assert_eq!(settings.onboarding_state().unwrap(), (false, false, true));

        settings.set("onboarding_completed", json!(true)).unwrap();
        assert_eq!(settings.onboarding_state().unwrap(), (true, false, false));
        assert!(settings.get("onboarding_in_progress").is_err());
    }

    #[test]
    fn existing_game_profiles_are_not_forced_into_onboarding() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(&temp.path().join("db")).unwrap();
        let settings = Settings::with_secret_store(db.clone(), Arc::new(MockSecrets::default()));
        let game = temp.path().join("legacy-game");
        std::fs::create_dir(&game).unwrap();
        crate::games::register_game(&db, &game, None).unwrap();
        assert_eq!(settings.onboarding_state().unwrap(), (false, false, false));
        assert_eq!(crate::games::list_games(&db).unwrap().len(), 1);
    }

    #[test]
    fn onboarding_key_is_rejected_when_unknown_and_legacy_databases_use_default() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("db")).unwrap();
        let settings = Settings::with_secret_store(db, Arc::new(MockSecrets::default()));
        assert!(settings
            .set("unknown_onboarding_flag", json!(true))
            .is_err());
        assert!(!settings.load().unwrap().onboarding_completed);
    }
}
