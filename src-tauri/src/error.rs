use serde::Serialize;

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

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
