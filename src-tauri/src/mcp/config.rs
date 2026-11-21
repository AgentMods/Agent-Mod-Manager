use crate::{
    db::Db,
    error::{AppError, AppResult},
    settings::Settings,
    util::now,
};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

// Windows Node distributions expose npx as a .cmd shim, not npx.exe.
const NPX_COMMAND: &str = if cfg!(windows) { "npx.cmd" } else { "npx" };

const PLAYWRIGHT_V2_ARGS: [&str; 5] = [
    "--yes",
    "@playwright/mcp@0.0.85",
    "--headless",
    "--isolated",
    "--no-webmcp",
];

fn playwright_args() -> Vec<String> {
    PLAYWRIGHT_V2_ARGS
        .into_iter()
        .chain(cfg!(windows).then_some("--browser=msedge"))
        .map(str::to_owned)
        .collect()
}

/// Schema v3: only the stock Windows launch changes; its old consent cannot be reused.
#[cfg(windows)]
pub(crate) fn migrate_playwright_browser(conn: &rusqlite::Connection) -> AppResult<()> {
    conn.execute(
        "UPDATE mcp_servers SET args_json=?1, startup_digest=NULL
         WHERE id='playwright' AND builtin=1 AND name='Playwright MCP'
           AND transport='stdio' AND command='npx.cmd' AND args_json=?2
           AND url IS NULL AND env_json='{}' AND secret_ref IS NULL",
        params![
            serde_json::to_string(&playwright_args())?,
            serde_json::to_string(&PLAYWRIGHT_V2_ARGS)?
        ],
    )?;
    Ok(())
}

pub(crate) const MARKITDOWN_ID: &str = "markitdown";

// Allow the specific prerelease required by MarkItDown's all extra, not all prereleases.
const MARKITDOWN_PACKAGES: [&str; 3] = [
    "--with",
    "azure-ai-contentunderstanding==1.2.0b3",
    "markitdown-mcp==0.0.1a8",
];

/// Schema v6: remove the explicit index from the unedited historical preset.
/// Keep this snapshot independent of future changes to the default packages or mirror.
pub(crate) fn migrate_markitdown_to_launcher_index(conn: &rusqlite::Connection) -> AppResult<()> {
    let previous = [
        "--default-index",
        "https://pypi.tuna.tsinghua.edu.cn/simple",
        "--with",
        "azure-ai-contentunderstanding==1.2.0b3",
        "markitdown-mcp==0.0.1a8",
    ];
    conn.execute(
        "UPDATE mcp_servers SET args_json=?1, startup_digest=NULL
         WHERE id='markitdown' AND builtin=1 AND name='MarkItDown MCP'
           AND transport='stdio' AND command='uvx' AND args_json=?2
           AND url IS NULL AND env_json='{}' AND secret_ref IS NULL",
        params![
            serde_json::to_string(&previous[2..])?,
            serde_json::to_string(&previous)?
        ],
    )?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    Stdio,
    StreamableHttp,
}

impl McpTransport {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::StreamableHttp => "streamable_http",
        }
    }
}

/// No plaintext env values, headers or passwords are accepted by this DTO.
/// Every env map value is an encrypted-Store reference, not its resolved value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub id: String,
    pub name: String,
    pub transport: McpTransport,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub env_secret_refs: BTreeMap<String, String>,
    pub bearer_secret_ref: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub builtin: bool,
}

impl ServerConfig {
    pub fn stdio(id: &str, name: &str, command: &str, args: Vec<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            transport: McpTransport::Stdio,
            command: Some(command.into()),
            args,
            url: None,
            env_secret_refs: BTreeMap::new(),
            bearer_secret_ref: None,
            enabled: false,
            builtin: false,
        }
    }

    pub fn http(id: &str, name: &str, url: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            transport: McpTransport::StreamableHttp,
            command: None,
            args: Vec::new(),
            url: Some(url.into()),
            env_secret_refs: BTreeMap::new(),
            bearer_secret_ref: None,
            enabled: false,
            builtin: false,
        }
    }

    // Stable for this DTO (env bindings are sorted); changed serialization fails closed.
    pub(crate) fn confirmation_digest(&self) -> AppResult<[u8; 32]> {
        Ok(Sha256::digest(serde_json::to_vec(self)?).into())
    }

    pub fn validate(&self) -> AppResult<()> {
        if self.id.is_empty()
            || self.id.len() > 48
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err(AppError::Invalid(
                "MCP server ID must be 1-48 ASCII letters, digits, _ or -".into(),
            ));
        }
        if self.name.trim().is_empty()
            || self.name.len() > 256
            || self.name.chars().any(char::is_control)
        {
            return Err(AppError::Invalid("Invalid MCP display name".into()));
        }
        if self.args.len() > 64
            || self
                .args
                .iter()
                .any(|v| v.len() > 4096 || v.chars().any(char::is_control))
        {
            return Err(AppError::Invalid("MCP arguments exceed limits".into()));
        }
        if self.env_secret_refs.len() > 16 {
            return Err(AppError::Invalid("Too many MCP secret bindings".into()));
        }
        for (env, key) in &self.env_secret_refs {
            // No loader/command injection via PATH, LD_PRELOAD, NODE_OPTIONS, etc.
            if !(env.ends_with("_API_KEY") || env.ends_with("_TOKEN"))
                || env.len() > 96
                || !env
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            {
                return Err(AppError::Invalid(
                    "Only *_API_KEY / *_TOKEN environment secret bindings are allowed".into(),
                ));
            }
            validate_secret_ref(key)?;
        }
        if let Some(key) = &self.bearer_secret_ref {
            validate_secret_ref(key)?;
        }
        match self.transport {
            McpTransport::Stdio => {
                let command = self.command.as_deref().unwrap_or_default();
                if command.is_empty()
                    || command.len() > 4096
                    || command.chars().any(char::is_control)
                    || self.url.is_some()
                    || self.bearer_secret_ref.is_some()
                {
                    return Err(AppError::Invalid("Invalid stdio MCP configuration".into()));
                }
            }
            McpTransport::StreamableHttp => {
                let url = url::Url::parse(self.url.as_deref().unwrap_or_default())
                    .map_err(|_| AppError::Invalid("Invalid MCP HTTP URL".into()))?;
                // Loopback HTTP permits local mocks/servers; remote credentials require TLS.
                let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
                if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                    || self.command.is_some()
                    || !self.args.is_empty()
                    || !self.env_secret_refs.is_empty()
                {
                    return Err(AppError::Invalid("MCP URL must be HTTPS (or loopback HTTP) without credentials, query or fragment".into()));
                }
            }
        }
        Ok(())
    }
}

fn validate_secret_ref(key: &str) -> AppResult<()> {
    if !key.starts_with("mcp.")
        || key.len() > 128
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(AppError::Invalid(
            "MCP secrets must reference a key named mcp.*".into(),
        ));
    }
    Ok(())
}

#[derive(Clone)]
pub struct McpServers {
    pub db: Db,
}

impl McpServers {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn list(&self) -> AppResult<Vec<ServerConfig>> {
        let conn = self.db.conn();
        let mut stmt = conn.prepare("SELECT id,name,transport,command,args_json,url,env_json,secret_ref,enabled,builtin FROM mcp_servers ORDER BY builtin DESC,name,id")?;
        let result = stmt
            .query_map([], from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(result)
    }

    pub fn get(&self, id: &str) -> AppResult<ServerConfig> {
        Self::get_from(&self.db.conn(), id)
    }

    fn get_from(conn: &rusqlite::Connection, id: &str) -> AppResult<ServerConfig> {
        conn.query_row("SELECT id,name,transport,command,args_json,url,env_json,secret_ref,enabled,builtin FROM mcp_servers WHERE id=?1", [id], from_row)
            .optional()?.ok_or_else(|| AppError::NotFound("MCP server".into()))
    }

    pub(crate) fn startup_requests(&self) -> AppResult<Vec<String>> {
        let conn = self.db.conn();
        let mut stmt = conn.prepare("SELECT id FROM mcp_servers WHERE startup_digest IS NOT NULL AND enabled=1 AND id<>?1 ORDER BY id")?;
        let ids = stmt
            .query_map([crate::mcp_connections::LOCAL_SERVER_ID], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    pub(super) fn startup_digest(&self, id: &str) -> AppResult<Option<Vec<u8>>> {
        Ok(self.db.conn().query_row(
            "SELECT startup_digest FROM mcp_servers WHERE id=?1",
            [id],
            |r| r.get(0),
        )?)
    }

    pub(crate) fn remember_start(&self, confirmed: &ServerConfig) -> AppResult<()> {
        let digest = confirmed.confirmation_digest()?;
        let conn = self.db.conn();
        let current = Self::get_from(&conn, &confirmed.id)?;
        if current != *confirmed {
            return Err(AppError::Conflict(
                "配置已变化，请重新确认完整连接配置".into(),
            ));
        }
        conn.execute(
            "UPDATE mcp_servers SET startup_digest=?2 WHERE id=?1",
            params![confirmed.id, digest],
        )?;
        Ok(())
    }

    pub(crate) fn forget_start(&self, id: &str) -> AppResult<()> {
        self.db.conn().execute(
            "UPDATE mcp_servers SET startup_digest=NULL WHERE id=?1",
            [id],
        )?;
        Ok(())
    }

    pub fn create(&self, config: &ServerConfig) -> AppResult<()> {
        self.insert(config, false)
    }

    fn insert(&self, config: &ServerConfig, builtin: bool) -> AppResult<()> {
        config.validate()?;
        self.db.conn().execute(
            "INSERT INTO mcp_servers(id,name,transport,command,args_json,url,env_json,secret_ref,enabled,builtin,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![config.id,config.name,config.transport.as_str(),config.command,serde_json::to_string(&config.args)?,config.url,
                serde_json::to_string(&config.env_secret_refs)?,config.bearer_secret_ref,config.enabled,builtin,now()],
        )?;
        Ok(())
    }

    pub fn update(&self, config: &ServerConfig) -> AppResult<()> {
        config.validate()?;
        let count = self.db.conn().execute(
            "UPDATE mcp_servers SET name=?2,transport=?3,command=?4,args_json=?5,url=?6,env_json=?7,secret_ref=?8,enabled=?9,startup_digest=NULL WHERE id=?1",
            params![config.id,config.name,config.transport.as_str(),config.command,serde_json::to_string(&config.args)?,config.url,
                serde_json::to_string(&config.env_secret_refs)?,config.bearer_secret_ref,config.enabled],
        )?;
        if count == 0 {
            return Err(AppError::NotFound("MCP server".into()));
        }
        Ok(())
    }

    /// Removes configuration only; encrypted credentials are deliberately retained.
    /// Existing clients detect this removal before subsequent tool invocations.
    pub fn remove(&self, id: &str) -> AppResult<()> {
        if self
            .db
            .conn()
            .execute("DELETE FROM mcp_servers WHERE id=?1", [id])?
            == 0
        {
            return Err(AppError::NotFound("MCP server".into()));
        }
        Ok(())
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> AppResult<()> {
        let changed = self.db.conn().execute(
            "UPDATE mcp_servers SET enabled=?2,startup_digest=NULL WHERE id=?1",
            params![id, enabled],
        )?;
        if changed == 0 {
            return Err(AppError::NotFound("MCP server".into()));
        }
        Ok(())
    }

    pub fn write_secret(&self, settings: &Settings, key: &str, value: &str) -> AppResult<()> {
        validate_secret_ref(key)?;
        settings.write_secret(key, value)
    }

    /// Insert-only seeding never overwrites edits or re-enables a disabled server.
    pub fn seed_presets(&self) -> AppResult<()> {
        let mut http = ServerConfig::http(
            "glossmod_http",
            "Mod客栈",
            "https://www.glossmod.com/api/v1/mcp",
        );
        http.bearer_secret_ref = Some("mcp.glossmod_http.api_key".into());
        let mut stdio = ServerConfig::stdio(
            "glossmod_3dm",
            "3DM MCP (glossmod-mcp)",
            "uvx",
            vec!["glossmod-mcp".into()],
        );
        stdio
            .env_secret_refs
            .insert("GLOSSMOD_API_KEY".into(), "mcp.glossmod_3dm.api_key".into());
        let playwright = ServerConfig::stdio(
            "playwright",
            "Playwright MCP",
            NPX_COMMAND,
            playwright_args(),
        );
        let markitdown = ServerConfig::stdio(
            MARKITDOWN_ID,
            "MarkItDown MCP",
            "uvx",
            MARKITDOWN_PACKAGES.map(String::from).into(),
        );
        for config in [http, stdio, playwright, markitdown] {
            match self.insert(&config, true) {
                Ok(()) => {}
                Err(AppError::Db(err))
                    if err.sqlite_error_code()
                        == Some(rusqlite::ErrorCode::ConstraintViolation) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<ServerConfig> {
    let decode = |index| -> rusqlite::Result<serde_json::Value> {
        let raw: String = row.get(index)?;
        serde_json::from_str(&raw).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })
    };
    let transport: String = row.get(2)?;
    let transport = match transport.as_str() {
        "stdio" => McpTransport::Stdio,
        "streamable_http" => McpTransport::StreamableHttp,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let convert = |index, err| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(err))
    };
    Ok(ServerConfig {
        id: row.get(0)?,
        name: row.get(1)?,
        transport,
        command: row.get(3)?,
        args: serde_json::from_value(decode(4)?).map_err(|err| convert(4, err))?,
        url: row.get(5)?,
        env_secret_refs: serde_json::from_value(decode(6)?).map_err(|err| convert(6, err))?,
        bearer_secret_ref: row.get(7)?,
        enabled: row.get(8)?,
        builtin: row.get(9)?,
    })
}
