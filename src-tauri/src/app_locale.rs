use crate::{
    db::Db,
    error::{AppError, AppResult},
};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

// This belongs to the installed app, not the importable Agent/model settings.
const SETTING_KEY: &str = "app_locale";
const LEGACY_KEY: &str = "mod_organization_locale";
const DEFAULT: &str = "en";
pub(crate) const LANGUAGES: &[(&str, &str)] = &[
    ("zh-CN", "简体中文"),
    ("en", "English"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("de", "Deutsch"),
    ("fr", "Français"),
    ("es", "Español"),
    ("pt-BR", "Português (Brasil)"),
    ("ru", "Русский"),
    ("it", "Italiano"),
];

fn supported(locale: &str) -> bool {
    LANGUAGES.iter().any(|&(code, _)| code == locale)
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct AppLocale {
    pub locale: Option<String>,
    pub effective_locale: String,
}

fn from_preferences(preferences: impl IntoIterator<Item = impl AsRef<str>>) -> &'static str {
    preferences
        .into_iter()
        .find_map(|preference| {
            // BCP 47 tags and locale strings such as pt_BR.UTF-8 both start with
            // the language. Ignore unsupported preferences before trying the next one.
            let primary = preference
                .as_ref()
                .trim()
                .split(['-', '_', '.', '@'])
                .next()?;
            LANGUAGES.iter().map(|&(code, _)| code).find(|locale| {
                locale
                    .split('-')
                    .next()
                    .is_some_and(|language| language.eq_ignore_ascii_case(primary))
            })
        })
        .unwrap_or(DEFAULT)
}

fn preference_response(locale: Option<String>) -> AppLocale {
    let effective_locale = locale
        .clone()
        .unwrap_or_else(|| from_preferences(sys_locale::get_locales()).into());
    AppLocale {
        locale,
        effective_locale,
    }
}

fn read_setting(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
            row.get(0)
        })
        .optional()?)
}

fn decode(saved: &str) -> Option<String> {
    serde_json::from_str::<String>(saved)
        .ok()
        .filter(|locale| supported(locale))
}

/// Consume the old per-feature preference once, without altering job or install records.
/// An existing unified value (including explicit System) always takes precedence.
pub(crate) fn migrate(db: &Db) -> AppResult<()> {
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    if let Some(old) = read_setting(&tx, LEGACY_KEY)? {
        if decode(&old).is_some() {
            tx.execute(
                "INSERT OR IGNORE INTO settings(key,value) VALUES(?1,?2)",
                (SETTING_KEY, old),
            )?;
        }
        tx.execute("DELETE FROM settings WHERE key=?1", [LEGACY_KEY])?;
    }
    tx.commit()?;
    Ok(())
}

pub fn get(db: &Db) -> AppResult<AppLocale> {
    let saved = read_setting(&db.conn(), SETTING_KEY)?;
    Ok(preference_response(saved.as_deref().and_then(decode)))
}

pub fn set(db: &Db, locale: Option<String>) -> AppResult<AppLocale> {
    if locale.as_deref().is_some_and(|locale| !supported(locale)) {
        return Err(AppError::Invalid(
            "不支持此应用语言，请从语言列表中选择".into(),
        ));
    }
    // Persist automatic mode as null so that a legacy writer cannot restore
    // an old override the next time the database is opened.
    db.conn().execute(
        "INSERT INTO settings(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        (SETTING_KEY, serde_json::to_string(&locale)?),
    )?;
    Ok(preference_response(locale))
}

/// Resolve the current language for new work. If the preference cannot be
/// read, use the same OS preference as a new installation.
pub fn effective(db: &Db) -> String {
    effective_conn(&db.conn())
}

/// For callers already holding the database connection (including transactions).
pub(crate) fn effective_conn(conn: &Connection) -> String {
    let saved = read_setting(conn, SETTING_KEY).ok().flatten();
    preference_response(saved.as_deref().and_then(decode)).effective_locale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_first_supported_primary_language() {
        for (requested, expected) in [
            ("zh-Hant-TW", "zh-CN"),
            ("en-GB", "en"),
            ("ja-JP", "ja"),
            ("ko-KR", "ko"),
            ("de-AT", "de"),
            ("FR-ca", "fr"),
            ("es-419", "es"),
            ("PT_pt.UTF-8", "pt-BR"),
            ("ru-RU", "ru"),
            ("it-CH", "it"),
        ] {
            assert_eq!(from_preferences(["ar-SA", requested, "en-US"]), expected);
        }
        assert_eq!(from_preferences(["ar-SA", "jp", "C.UTF-8"]), "en");
        assert_eq!(from_preferences(std::iter::empty::<&str>()), "en");
    }

    #[test]
    fn manual_and_system_preferences_persist_without_touching_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.sqlite");
        let db = Db::open(&path).unwrap();
        assert_eq!(get(&db).unwrap().locale, None);
        db.conn()
            .execute(
                "INSERT INTO settings(key,value) VALUES('onboarding_completed','true')",
                [],
            )
            .unwrap();
        let japanese = set(&db, Some("ja".into())).unwrap();
        assert_eq!(
            serde_json::to_value(&japanese).unwrap(),
            serde_json::json!({
                "locale": "ja", "effective_locale": "ja",
            })
        );
        assert!(set(&db, Some("ja-JP".into())).is_err());
        assert_eq!(effective(&db), "ja");
        drop(db);

        let db = Db::open(&path).unwrap();
        assert_eq!(get(&db).unwrap(), japanese);
        assert_eq!(set(&db, None).unwrap(), get(&db).unwrap());
        assert_eq!(get(&db).unwrap().locale, None);
        assert!(supported(&effective(&db)));
        let rows: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM settings WHERE key IN (?1,'onboarding_completed')",
                [SETTING_KEY],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rows, 2);
        assert_eq!(
            read_setting(&db.conn(), SETTING_KEY).unwrap().as_deref(),
            Some("null")
        );
    }

    #[test]
    fn an_unrecognized_saved_value_is_treated_as_automatic() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("app.sqlite")).unwrap();
        db.conn()
            .execute(
                "INSERT INTO settings(key,value) VALUES(?1,'unknown')",
                [SETTING_KEY],
            )
            .unwrap();
        assert_eq!(get(&db).unwrap(), preference_response(None));
    }

    #[test]
    fn migration_consumes_supported_legacy_override_and_cannot_restore_it_after_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.sqlite");
        let write_legacy = |db: &Db| {
            db.conn()
                .execute(
                    "INSERT INTO settings(key,value) VALUES(?1,?2)",
                    (LEGACY_KEY, serde_json::to_string("pt-BR").unwrap()),
                )
                .unwrap()
        };
        write_legacy(&Db::open(&path).unwrap());
        let db = Db::open(&path).unwrap();
        assert_eq!(get(&db).unwrap().locale.as_deref(), Some("pt-BR"));
        assert_eq!(read_setting(&db.conn(), LEGACY_KEY).unwrap(), None);
        set(&db, None).unwrap();
        write_legacy(&db);
        drop(db);
        let db = Db::open(&path).unwrap();
        assert_eq!(get(&db).unwrap().locale, None);
        assert_eq!(read_setting(&db.conn(), LEGACY_KEY).unwrap(), None);
    }

    #[test]
    fn unsupported_legacy_languages_return_to_system_and_manual_unified_choice_wins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.sqlite");
        let db = Db::open(&path).unwrap();
        for legacy in ["zh-TW", "pl", "uk", "tr", "ja-JP"] {
            db.conn()
                .execute(
                    "INSERT INTO settings(key,value) VALUES(?1,?2)",
                    (LEGACY_KEY, serde_json::to_string(legacy).unwrap()),
                )
                .unwrap();
            migrate(&db).unwrap();
            assert_eq!(get(&db).unwrap().locale, None);
            assert_eq!(read_setting(&db.conn(), LEGACY_KEY).unwrap(), None);
        }
        set(&db, Some("de".into())).unwrap();
        db.conn()
            .execute(
                "INSERT INTO settings(key,value) VALUES(?1,?2)",
                (LEGACY_KEY, serde_json::to_string("ja").unwrap()),
            )
            .unwrap();
        migrate(&db).unwrap();
        assert_eq!(effective(&db), "de");
    }
}
