use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    sync::LazyLock,
};

#[derive(Deserialize)]
struct Translation {
    #[serde(rename = "zh-CN")]
    fallback: String,
    // Exact compatibility mapping for existing typed domain errors. No parsing
    // of arbitrary paths, vendor text or error strings takes place.
    legacy: Option<String>,
}

static CATALOG: LazyLock<HashMap<String, Translation>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../locales/native.json"))
        .expect("valid native message catalog")
});

/// Presentation-only data for native errors and warnings. `message` is the
/// original readable fallback for consumers without the matching catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeMessage {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub args: BTreeMap<String, String>,
    pub message: String,
}

impl NativeMessage {
    pub fn new<const N: usize>(key: &str, args: [(&str, String); N]) -> Self {
        let args: BTreeMap<String, String> = args
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect();
        let template = &CATALOG.get(key).expect("known native message key").fallback;
        let mut message = String::new();
        let mut rest = template.as_str();
        // Single-pass substitution: inserted paths or external errors cannot be
        // interpreted as more placeholders.
        while let Some((prefix, after_open)) = rest.split_once('{') {
            let Some((name, tail)) = after_open.split_once('}') else {
                break;
            };
            message.push_str(prefix);
            if let Some(value) = args.get(name) {
                message.push_str(value);
            } else {
                message.push('{');
                message.push_str(name);
                message.push('}');
            }
            rest = tail;
        }
        message.push_str(rest);
        Self {
            key: key.into(),
            args,
            message,
        }
    }

    fn fallback(key: &str, message: String) -> Self {
        Self {
            key: key.into(),
            args: BTreeMap::new(),
            message,
        }
    }
}

impl fmt::Display for NativeMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("数据库错误: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("网络错误: {0}")]
    Net(#[from] reqwest::Error),
    #[error("序列化错误: {0}")]
    Json(#[from] serde_json::Error),
    #[error("下载 DNS 解析失败: {0}")]
    DownloadDns(String),
    #[error("路径越界: {0}")]
    PathEscape(String),
    #[error("不安全归档: {0}")]
    UnsafeArchive(String),
    #[error("未找到: {0}")]
    NotFound(String),
    #[error("无效输入: {0}")]
    Invalid(String),
    #[error("权限被拒绝: {0}")]
    Denied(String),
    #[error("冲突: {0}")]
    Conflict(String),
    #[error("不支持: {0}")]
    Unsupported(String),
    #[error("已取消")]
    Cancelled,
    /// The model provider rejected the request for exceeding its context window.
    #[error("{0}")]
    ContextLength(String),
    #[error("{0}")]
    Other(String),
    #[error("{0}")]
    Presented(NativeMessage),
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Other(e.to_string())
    }
}

impl From<zip::result::ZipError> for AppError {
    fn from(e: zip::result::ZipError) -> Self {
        AppError::UnsafeArchive(e.to_string())
    }
}

impl From<walkdir::Error> for AppError {
    fn from(e: walkdir::Error) -> Self {
        AppError::Io(std::io::Error::other(e.to_string()))
    }
}

impl AppError {
    pub fn presentation(&self) -> NativeMessage {
        let (category, detail) = match self {
            Self::Db(error) => ("error.db", error.to_string()),
            Self::Io(error) => ("error.io", error.to_string()),
            Self::Net(error) => ("error.net", error.to_string()),
            Self::Json(error) => ("error.json", error.to_string()),
            Self::DownloadDns(detail) => ("error.download_dns", detail.clone()),
            Self::PathEscape(detail) => ("error.path_escape", detail.clone()),
            Self::UnsafeArchive(detail) => ("error.unsafe_archive", detail.clone()),
            Self::NotFound(detail) => ("error.not_found", detail.clone()),
            Self::Invalid(detail) => ("error.invalid", detail.clone()),
            Self::Denied(detail) => ("error.denied", detail.clone()),
            Self::Conflict(detail) => ("error.conflict", detail.clone()),
            Self::Unsupported(detail) => ("error.unsupported", detail.clone()),
            Self::Cancelled => return NativeMessage::new("error.cancelled", []),
            Self::ContextLength(detail) => {
                return NativeMessage::fallback("error.raw", detail.clone())
            }
            Self::Other(detail) => ("error.raw", detail.clone()),
            Self::Presented(message) => return message.clone(),
        };
        if let Some((key, _)) = CATALOG.iter().find(|(_, entry)| {
            entry.legacy.as_deref() == Some(category) && entry.fallback == detail
        }) {
            return NativeMessage::fallback(key, self.to_string());
        }
        if category == "error.raw" {
            NativeMessage::fallback(category, detail)
        } else {
            NativeMessage::new(category, [("detail", detail)])
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.presentation().serialize(s)
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::collections::BTreeSet;

    #[test]
    fn native_error_serialization_preserves_details_and_display_fallback() {
        let error = AppError::PathEscape("C:/Mods/{detail}".into());
        assert_eq!(
            serde_json::to_value(&error).unwrap(),
            json!({
                "key": "error.path_escape", "args": { "detail": "C:/Mods/{detail}" },
                "message": error.to_string(),
            })
        );
        let error = AppError::Invalid("不支持此应用语言，请从语言列表中选择".into());
        assert_eq!(
            serde_json::to_value(&error).unwrap(),
            json!({
                "key": "locale.unsupported", "args": {}, "message": error.to_string(),
            })
        );
        let raw = AppError::Other("vendor error {platform}".into());
        assert_eq!(
            serde_json::to_value(&raw).unwrap(),
            json!({
                "key": "error.raw", "args": {}, "message": raw.to_string(),
            })
        );
    }

    #[test]
    fn native_catalog_has_all_locales_and_consistent_placeholders() {
        fn fields(value: &str) -> BTreeSet<&str> {
            value
                .split('{')
                .skip(1)
                .map(|rest| rest.split_once('}').unwrap().0)
                .collect()
        }
        let catalog: HashMap<String, Value> =
            serde_json::from_str(include_str!("../locales/native.json")).unwrap();
        let mut legacy_pairs = BTreeSet::new();
        for (key, entry) in &catalog {
            let fallback = entry["zh-CN"].as_str().unwrap();
            let expected_fields = fields(fallback);
            for &(locale, _) in crate::app_locale::LANGUAGES {
                let value = entry[locale]
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| panic!("missing {key} translation for {locale}"));
                assert_eq!(fields(value), expected_fields, "{key} ({locale})");
            }
            if let Some(category) = entry["legacy"].as_str() {
                assert!(
                    expected_fields.is_empty(),
                    "legacy messages have no template args"
                );
                assert!(
                    legacy_pairs.insert((category, fallback)),
                    "ambiguous legacy mapping: {key}"
                );
            }
        }
    }
}
