use crate::{
    db::Db,
    error::{AppError, AppResult},
    settings::Settings,
    util::now,
};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf};

// Historical Playwright launchers used the platform-specific npx shim.
const LEGACY_PLAYWRIGHT_COMMAND: &str = if cfg!(windows) { "npx.cmd" } else { "npx" };
const LEGACY_PLAYWRIGHT_DESCRIPTION: &str = "在独立的浏览器会话中打开、浏览和操作网页。";
pub(super) const PLAYWRIGHT_COMMAND: &str = "amm-playwright";
const PLAYWRIGHT_DESCRIPTION: &str =
    "在独立的浏览器会话中打开、浏览和操作网页。首次连接时自动下载 Chromium（若尚未安装），后续复用。";

fn playwright_args(package: &str, edge: bool) -> Vec<String> {
    ["--yes", package, "--headless", "--isolated", "--no-webmcp"]
        .into_iter()
        .chain(edge.then_some("--browser=msedge"))
        .map(str::to_owned)
        .collect()
}

// Snapshot shared by the v11 and v12 migrations. No download flags here.
fn playwright_v11_args() -> Vec<String> {
    playwright_args("@playwright/mcp@latest", cfg!(windows))
}

pub(super) fn playwright_output_dir(bases: impl IntoIterator<Item = PathBuf>) -> Option<String> {
    bases
        .into_iter()
        .filter(|base| base.is_absolute())
        .find_map(|base| {
            base.join("Agent Mod Manager")
                .join("Playwright")
                .into_os_string()
                .into_string()
                .ok()
        })
}

fn default_playwright_output_dir() -> Option<String> {
    // Resolve the user's Downloads known folder (which may be redirected), not
    // an install/cwd-relative directory. Playwright creates the directory on use.
    playwright_output_dir(
        [
            dirs::download_dir(),
            dirs::data_local_dir(),
            dirs::home_dir(),
        ]
        .into_iter()
        .flatten()
        .chain(std::iter::once_with(std::env::temp_dir)),
    )
}

fn playwright_v12_args(output_dir: String) -> Vec<String> {
    let mut args = playwright_v11_args();
    args.extend([
        "--file-paths=absolute".into(),
        "--output-dir".into(),
        output_dir,
    ]);
    args
}

fn playwright_v17_args(output_dir: String) -> Vec<String> {
    vec![
        "--browser=chromium".into(),
        "--headless".into(),
        "--sandbox".into(),
        "--isolated".into(),
        "--no-webmcp".into(),
        "--file-paths=absolute".into(),
        "--output-dir".into(),
        output_dir,
    ]
}

fn require_default_playwright_output_dir() -> AppResult<String> {
    default_playwright_output_dir()
        .ok_or_else(|| AppError::Invalid("无法确定 Playwright MCP 下载目录的绝对路径".into()))
}

fn stock_playwright_receipt_dir(config: &ServerConfig, expected_dir: String) -> Option<PathBuf> {
    // Treat the stored MCP settings as configuration, not filesystem authority.
    // Only the unmodified shipped launch and independently resolved output path
    // qualify. Never accept arbitrary --output-dir, env or launcher overrides.
    (config.id == PLAYWRIGHT_ID
        && config.builtin
        && config.plugin_slug.is_none()
        && config.name == "Playwright MCP"
        && config.transport == McpTransport::Stdio
        && config.command.as_deref() == Some(PLAYWRIGHT_COMMAND)
        && config.url.is_none()
        && config.credential_url.is_none()
        && config.env_secret_refs.is_empty()
        && config.bearer_secret_ref.is_none()
        && config.args == playwright_v17_args(expected_dir.clone()))
    .then(|| PathBuf::from(expected_dir))
}

/// Only stock Playwright may supply an external receipt folder. Customized
/// launchers and relocated output require the explicit desktop picker.
pub(crate) fn playwright_receipt_dir(db: &Db) -> AppResult<Option<PathBuf>> {
    let Some(expected_dir) = default_playwright_output_dir() else {
        return Ok(None);
    };
    match McpServers::new(db.clone()).get(PLAYWRIGHT_ID) {
        Ok(config) => Ok(stock_playwright_receipt_dir(&config, expected_dir)),
        Err(AppError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Schema v12: only the stock v11 Playwright command gains an output path.
pub(crate) fn migrate_playwright_downloads(conn: &Connection) -> AppResult<()> {
    let args = playwright_v12_args(require_default_playwright_output_dir()?);
    conn.execute(
        "UPDATE mcp_servers SET args_json=?1, startup_digest=NULL
         WHERE id='playwright' AND builtin=1 AND plugin_slug IS NULL AND name='Playwright MCP'
           AND transport='stdio' AND command=?2 AND args_json=?3
           AND url IS NULL AND env_json='{}' AND secret_ref IS NULL",
        params![
            serde_json::to_string(&args)?,
            LEGACY_PLAYWRIGHT_COMMAND,
            serde_json::to_string(&playwright_v11_args())?
        ],
    )?;
    Ok(())
}

/// Schema v17: switch only the stock v12 launch to the bundled application alias.
/// Keep enablement and custom descriptions, but require new auto-start consent.
pub(crate) fn migrate_bundled_playwright(conn: &Connection) -> AppResult<()> {
    // Without an independently resolved default, there is no exact stock path to match.
    let Some(output_dir) = default_playwright_output_dir() else {
        return Ok(());
    };
    let previous_args = playwright_v12_args(output_dir.clone());
    let bundled_args = playwright_v17_args(output_dir);
    conn.execute(
        "UPDATE mcp_servers SET command=?1, args_json=?2, startup_digest=NULL,
           description=CASE WHEN description=?5 THEN ?6 ELSE description END
         WHERE id='playwright' AND builtin=1 AND plugin_slug IS NULL AND name='Playwright MCP'
           AND transport='stdio' AND command=?3 AND args_json=?4
           AND url IS NULL AND credential_url IS NULL AND env_json='{}' AND secret_ref IS NULL",
        params![
            PLAYWRIGHT_COMMAND,
            serde_json::to_string(&bundled_args)?,
            LEGACY_PLAYWRIGHT_COMMAND,
            serde_json::to_string(&previous_args)?,
            LEGACY_PLAYWRIGHT_DESCRIPTION,
            PLAYWRIGHT_DESCRIPTION
        ],
    )?;
    Ok(())
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
            serde_json::to_string(&playwright_args("@playwright/mcp@0.0.85", true))?,
            serde_json::to_string(&playwright_args("@playwright/mcp@0.0.85", false))?
        ],
    )?;
    Ok(())
}

pub(crate) const MARKITDOWN_ID: &str = "markitdown";

// MarkItDown currently needs a prerelease of the Azure content-understanding dependency.
const MARKITDOWN_PACKAGES: [&str; 3] = ["--prerelease", "allow", "markitdown-mcp@latest"];

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

const PLAYWRIGHT_ID: &str = "playwright";
const GLOSSMOD_HTTP_ID: &str = "glossmod_http";
const GLOSSMOD_HTTP_ENDPOINT: &str = "https://www.glossmod.com/api/v1/mcp";
const GLOSSMOD_3DM_ID: &str = "glossmod_3dm";
const GLOSSMOD_HTTP_CREDENTIAL_URL: &str = "https://www.glossmod.com/creator/ai";
const GLOSSMOD_3DM_ENDPOINT: &str = "https://mod.3dmgame.com/mcp";
const GLOSSMOD_3DM_CREDENTIAL_URL: &str = "https://mod.3dmgame.com/Workshop/Api";

const MODIO_ID: &str = "modio";
pub(super) const NEXUSMODS_ID: &str = "nexusmods";

struct ModSourcePreset {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    args: &'static [&'static str],
    credential_url: Option<&'static str>,
    // (environment variable, suffix of this preset's encrypted-Store reference)
    bindings: &'static [(&'static str, &'static str)],
}

const MOD_SOURCE_PRESETS: &[ModSourcePreset] = &[
    ModSourcePreset {
        id: MODIO_ID,
        name: "mod.io MCP",
        description: "查找 mod.io 上的游戏与模组，查看模组和文件详情。",
        args: &["modio-mcp@latest"],
        credential_url: Some("https://mod.io/me/access"),
        bindings: &[
            ("MODIO_API_BASE_URL", "api_base_url"),
            ("MODIO_API_KEY", "api_key"),
        ],
    },
    ModSourcePreset {
        id: "curseforge",
        name: "CurseForge MCP",
        description: "搜索 CurseForge 模组，查询项目、版本与文件信息。",
        args: &["curseforge-mcp@latest"],
        credential_url: Some("https://console.curseforge.com/#/api-keys"),
        bindings: &[("CURSEFORGE_API_KEY", "api_key")],
    },
    ModSourcePreset {
        id: "gamebanana",
        name: "GameBanana MCP",
        description: "浏览和搜索 GameBanana 模组，查看条目与下载文件。",
        args: &["gamebanana-mcp@latest"],
        credential_url: None,
        bindings: &[],
    },
    ModSourcePreset {
        id: NEXUSMODS_ID,
        name: "Nexus Mods MCP",
        description: "查找 Nexus Mods 模组，查看项目和文件信息。",
        args: &[
            "--upgrade",
            "--from",
            "glossmod-nexusmods-mcp",
            "nexusmods-mcp",
        ],
        credential_url: Some("https://www.nexusmods.com/settings/api-keys"),
        bindings: &[("NEXUSMODS_API_KEY", "api_key")],
    },
];

impl ModSourcePreset {
    fn config(&self) -> ServerConfig {
        let args = self.args.iter().map(|&arg| arg.to_owned()).collect();
        let mut config = ServerConfig::stdio(self.id, self.name, "uvx", args);
        config.description = Some(self.description.into());
        config.credential_url = self.credential_url.map(str::to_owned);
        config.env_secret_refs = self
            .bindings
            .iter()
            .map(|&(env, suffix)| (env.to_owned(), format!("mcp.{}.{suffix}", self.id)))
            .collect();
        config
    }
}

fn builtin_presets(
    playwright_command: &str,
    playwright_args: Vec<String>,
    playwright_description: &str,
) -> impl Iterator<Item = ServerConfig> {
    let mut http = ServerConfig::http(GLOSSMOD_HTTP_ID, "Mod客栈", GLOSSMOD_HTTP_ENDPOINT);
    http.description = Some("搜索 Mod 客栈的模组，查看项目与下载信息。".into());
    http.credential_url = Some(GLOSSMOD_HTTP_CREDENTIAL_URL.into());
    http.bearer_secret_ref = Some("mcp.glossmod_http.api_key".into());
    let mut three_dm = ServerConfig::http(GLOSSMOD_3DM_ID, "3DM MCP", GLOSSMOD_3DM_ENDPOINT);
    three_dm.description = Some("搜索 3DM Mod 站的模组，查看项目与下载信息。".into());
    three_dm.credential_url = Some(GLOSSMOD_3DM_CREDENTIAL_URL.into());
    three_dm.bearer_secret_ref = Some("mcp.glossmod_3dm.api_key".into());
    let mut playwright = ServerConfig::stdio(
        PLAYWRIGHT_ID,
        "Playwright MCP",
        playwright_command,
        playwright_args,
    );
    playwright.description = Some(playwright_description.into());
    let mut markitdown = ServerConfig::stdio(
        MARKITDOWN_ID,
        "MarkItDown MCP",
        "uvx",
        MARKITDOWN_PACKAGES.map(String::from).into(),
    );
    markitdown.description = Some("将 PDF、Office 文档和网页等内容转换为 Markdown 文本。".into());
    [http, three_dm, playwright, markitdown]
        .into_iter()
        .chain(MOD_SOURCE_PRESETS.iter().map(ModSourcePreset::config))
}

fn v11_builtin_presets() -> impl Iterator<Item = ServerConfig> {
    builtin_presets(
        LEGACY_PLAYWRIGHT_COMMAND,
        playwright_v11_args(),
        LEGACY_PLAYWRIGHT_DESCRIPTION,
    )
}

/// Schema v11: replace only the last shipped package commands. A changed command
/// requires a fresh Start click before it can be restored at application startup.
pub(crate) fn migrate_unpinned_presets(conn: &Connection) -> AppResult<()> {
    for preset in v11_builtin_presets() {
        let previous: Vec<String> = match preset.id.as_str() {
            PLAYWRIGHT_ID => playwright_args("@playwright/mcp@0.0.85", cfg!(windows)),
            MARKITDOWN_ID => [
                "--with",
                "azure-ai-contentunderstanding==1.2.0b3",
                "markitdown-mcp==0.0.1a8",
            ]
            .map(String::from)
            .into(),
            MODIO_ID => vec!["modio-mcp==1.0.0".into()],
            "curseforge" => vec!["curseforge-mcp==1.0.0".into()],
            "gamebanana" => vec!["gamebanana-mcp==1.0.0".into()],
            NEXUSMODS_ID => ["--from", "glossmod-nexusmods-mcp==1.0.0", "nexusmods-mcp"]
                .map(String::from)
                .into(),
            _ => continue,
        };
        conn.execute(
            "UPDATE mcp_servers SET args_json=?1, startup_digest=NULL
             WHERE id=?2 AND name=?3 AND command=?4 AND args_json=?5
               AND builtin=1 AND plugin_slug IS NULL AND transport='stdio'",
            params![
                serde_json::to_string(&preset.args)?,
                preset.id,
                preset.name,
                preset.command,
                serde_json::to_string(&previous)?
            ],
        )?;
    }
    Ok(())
}

/// Schema v10: populate display metadata without changing configuration or consent.
pub(crate) fn migrate_descriptions(conn: &Connection) -> AppResult<()> {
    for config in v11_builtin_presets() {
        conn.execute(
            "UPDATE mcp_servers SET description=?2 WHERE id=?1 AND builtin=1 AND description IS NULL",
            params![config.id, config.description],
        )?;
    }
    Ok(())
}

/// Schema v9: switch the stock 3DM service to HTTP and expire its stdio consent.
pub(crate) fn migrate_3dm_to_http(conn: &Connection) -> AppResult<()> {
    conn.execute(
        r#"UPDATE mcp_servers SET name='3DM MCP', transport='streamable_http', command=NULL, args_json='[]',
             url='https://mod.3dmgame.com/mcp', env_json='{}', secret_ref='mcp.glossmod_3dm.api_key', startup_digest=NULL
           WHERE id='glossmod_3dm' AND builtin=1 AND plugin_slug IS NULL
             AND name IN ('3DM MCP', '3DM MCP (glossmod-mcp)')
             AND transport='stdio' AND command='uvx' AND args_json='["glossmod-mcp"]'
             AND url IS NULL AND secret_ref IS NULL
             AND env_json='{"GLOSSMOD_API_KEY":"mcp.glossmod_3dm.api_key"}'"#,
        [],
    )?;
    Ok(())
}

/// Schema v7: backfill the credential page for built-ins without changing their launch config.
pub(crate) fn migrate_credential_urls(conn: &rusqlite::Connection) -> AppResult<()> {
    for (id, url) in [
        (GLOSSMOD_HTTP_ID, GLOSSMOD_HTTP_CREDENTIAL_URL),
        (GLOSSMOD_3DM_ID, GLOSSMOD_3DM_CREDENTIAL_URL),
    ] {
        conn.execute(
            "UPDATE mcp_servers SET credential_url=?2
             WHERE id=?1 AND builtin=1 AND credential_url IS NULL",
            params![id, url],
        )?;
    }
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
    /// Optional display text; not part of launch consent or authorization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub transport: McpTransport,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub url: Option<String>,
    /// Optional web page for obtaining credentials; never used as a transport endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_url: Option<String>,
    #[serde(default)]
    pub env_secret_refs: BTreeMap<String, String>,
    pub bearer_secret_ref: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub builtin: bool,
    /// Plugin ownership is assigned by the registry, never by manual CRUD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_slug: Option<String>,
}

impl ServerConfig {
    pub(super) fn is_glossmod_endpoint(&self) -> bool {
        self.matches_http_endpoint(GLOSSMOD_HTTP_ENDPOINT)
    }

    pub(super) fn uses_3dm_authorization(&self) -> bool {
        self.matches_http_endpoint(GLOSSMOD_3DM_ENDPOINT)
    }

    fn matches_http_endpoint(&self, endpoint: &str) -> bool {
        self.transport == McpTransport::StreamableHttp
            && self
                .url
                .as_deref()
                .and_then(|s| url::Url::parse(s).ok())
                .is_some_and(|url| {
                    url.as_str() == endpoint || url.as_str().strip_suffix('/') == Some(endpoint)
                })
    }

    pub fn stdio(id: &str, name: &str, command: &str, args: Vec<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            transport: McpTransport::Stdio,
            command: Some(command.into()),
            args,
            url: None,
            credential_url: None,
            env_secret_refs: BTreeMap::new(),
            bearer_secret_ref: None,
            enabled: false,
            builtin: false,
            plugin_slug: None,
        }
    }

    pub fn http(id: &str, name: &str, url: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            transport: McpTransport::StreamableHttp,
            command: None,
            args: Vec::new(),
            url: Some(url.into()),
            credential_url: None,
            env_secret_refs: BTreeMap::new(),
            bearer_secret_ref: None,
            enabled: false,
            builtin: false,
            plugin_slug: None,
        }
    }

    /// Keep the pre-description exact match for live connections and pending consent.
    /// Unlike the persisted digest, this also checks the credential page, as before.
    pub(crate) fn same_connection_config(&self, other: &Self) -> bool {
        if self.description == other.description {
            return self == other;
        }
        // Compare all fields via derived equality; only description is normalized.
        let mut config = self.clone();
        config.description.clone_from(&other.description);
        config == *other
    }

    // Keep the pre-v7 digest: neither the credential page nor description is a launch setting.
    // Other serialized fields (including plugin ownership, when present) participate.
    pub(crate) fn confirmation_digest(&self) -> AppResult<[u8; 32]> {
        let mut launch_config = self.clone();
        launch_config.credential_url = None;
        launch_config.description = None;
        Ok(Sha256::digest(serde_json::to_vec(&launch_config)?).into())
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
            let modio_endpoint = self.id == MODIO_ID && env == "MODIO_API_BASE_URL";
            if !(env.ends_with("_API_KEY") || env.ends_with("_TOKEN") || modio_endpoint)
                || env.len() > 96
                || !env
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            {
                return Err(AppError::Invalid(
                    "MCP 环境变量只支持 *_API_KEY、*_TOKEN 和 mod.io 预设的 MODIO_API_BASE_URL"
                        .into(),
                ));
            }
            validate_secret_ref(key)?;
        }
        if let Some(key) = &self.bearer_secret_ref {
            validate_secret_ref(key)?;
        }
        if let Some(url) = &self.credential_url {
            validate_credential_url(url)?;
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

fn validate_credential_url(raw: &str) -> AppResult<()> {
    let invalid = || {
        AppError::Invalid(
            "MCP credential URL must be an absolute HTTP(S) URL without user info".into(),
        )
    };
    // The URL parser normalizes some inputs (missing slashes, backslashes, empty user info).
    // Require an explicit authority and reject those spellings before parsing.
    let (_, rest) = raw.split_once("://").ok_or_else(invalid)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty()
        || authority.contains('@')
        || raw
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace() || ch == '\\')
    {
        return Err(invalid());
    }
    let url = url::Url::parse(raw).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid());
    }
    Ok(())
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

const SERVER_COLUMNS: &str = "id,name,transport,command,args_json,url,env_json,secret_ref,enabled,builtin,credential_url,plugin_slug,description";
const OWNER_ACTIVE: &str = "(plugin_slug IS NULL OR EXISTS (
    SELECT 1 FROM plugins WHERE plugins.slug=mcp_servers.plugin_slug AND plugins.enabled=1 AND plugins.validated=1
))";

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
        let mut stmt = conn.prepare(&format!(
            "SELECT {SERVER_COLUMNS} FROM mcp_servers WHERE {OWNER_ACTIVE} ORDER BY builtin DESC,name,id"
        ))?;
        let result = stmt
            .query_map([], from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(result)
    }

    pub fn get(&self, id: &str) -> AppResult<ServerConfig> {
        Self::get_from(&self.db.conn(), id)
    }

    // Read the config and its effective availability from the same SQLite snapshot.
    // Keep an inactive owned config readable, but never present it as enabled to callers.
    fn get_from(conn: &Connection, id: &str) -> AppResult<ServerConfig> {
        conn.query_row(
            &format!("SELECT {SERVER_COLUMNS}, {OWNER_ACTIVE} FROM mcp_servers WHERE id=?1"),
            [id],
            |row| {
                let mut config = from_row(row)?;
                config.enabled &= row.get::<_, bool>(13)?;
                Ok(config)
            },
        )
        .optional()?
        .ok_or_else(|| AppError::NotFound("MCP server".into()))
    }

    pub(crate) fn startup_requests(&self) -> AppResult<Vec<String>> {
        let conn = self.db.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT id FROM mcp_servers WHERE startup_digest IS NOT NULL AND enabled=1 AND id<>?1
             AND {OWNER_ACTIVE} ORDER BY id"
        ))?;
        let ids = stmt
            .query_map([crate::mcp_connections::LOCAL_SERVER_ID], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    pub(super) fn startup_digest(&self, id: &str) -> AppResult<Option<Vec<u8>>> {
        Ok(self.db.conn().query_row(
            &format!("SELECT CASE WHEN {OWNER_ACTIVE} THEN startup_digest END FROM mcp_servers WHERE id=?1"),
            [id],
            |r| r.get(0),
        )?)
    }

    pub(crate) fn remember_start(&self, confirmed: &ServerConfig) -> AppResult<()> {
        let digest = confirmed.confirmation_digest()?;
        let conn = self.db.conn();
        let current = Self::get_from(&conn, &confirmed.id)?;
        if !current.same_connection_config(confirmed) {
            return Err(AppError::Conflict(
                "配置已变化，请重新确认完整连接配置".into(),
            ));
        }
        let changed = conn.execute(
            &format!(
                "UPDATE mcp_servers SET startup_digest=?2 WHERE id=?1
                      AND (plugin_slug IS NULL OR (enabled=1 AND {OWNER_ACTIVE}))"
            ),
            params![confirmed.id, digest],
        )?;
        if changed == 0 {
            return Err(inactive_plugin_error());
        }
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
        require_manual_config(config)?;
        config.validate()?;
        insert_server(&self.db.conn(), config, builtin).map(|_| ())
    }

    /// Returns whether the connection must be revoked. A description-only edit
    /// must keep both the saved launch consent and the live connection intact.
    pub fn update(&self, config: &ServerConfig) -> AppResult<bool> {
        require_manual_config(config)?;
        config.validate()?;
        let conn = self.db.conn();
        let current = Self::get_from(&conn, &config.id)?;
        require_manual_config(&current)?;
        if current.same_connection_config(config) {
            if current.description != config.description {
                conn.execute(
                    "UPDATE mcp_servers SET description=?2 WHERE id=?1 AND plugin_slug IS NULL",
                    params![config.id, config.description],
                )?;
            }
            return Ok(false);
        }
        let count = conn.execute(
            "UPDATE mcp_servers SET name=?2,transport=?3,command=?4,args_json=?5,url=?6,env_json=?7,secret_ref=?8,enabled=?9,credential_url=?10,description=?11,startup_digest=NULL WHERE id=?1 AND plugin_slug IS NULL",
            params![config.id,config.name,config.transport.as_str(),config.command,serde_json::to_string(&config.args)?,config.url,
                serde_json::to_string(&config.env_secret_refs)?,config.bearer_secret_ref,config.enabled,config.credential_url,config.description],
        )?;
        manual_mutation_result(&conn, &config.id, count)?;
        Ok(true)
    }

    /// Removes configuration only; encrypted credentials are deliberately retained.
    /// Existing clients detect this removal before subsequent tool invocations.
    pub fn remove(&self, id: &str) -> AppResult<()> {
        let conn = self.db.conn();
        let count = conn.execute(
            "DELETE FROM mcp_servers WHERE id=?1 AND plugin_slug IS NULL",
            [id],
        )?;
        manual_mutation_result(&conn, id, count)
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> AppResult<()> {
        let conn = self.db.conn();
        // The condition is part of the write, so a concurrent plugin disable cannot
        // slip in between checking ownership and enabling the service.
        let changed = conn.execute(
            &format!(
                "UPDATE mcp_servers SET enabled=?2,startup_digest=NULL WHERE id=?1
                      AND (?2=0 OR {OWNER_ACTIVE})"
            ),
            params![id, enabled],
        )?;
        if changed != 0 {
            Ok(())
        } else if has_server(&conn, id)? {
            Err(inactive_plugin_error())
        } else {
            Err(AppError::NotFound("MCP server".into()))
        }
    }

    pub fn write_secret(&self, settings: &Settings, key: &str, value: &str) -> AppResult<()> {
        validate_secret_ref(key)?;
        settings.write_secret(key, value)
    }

    /// Insert-only seeding never overwrites edits or re-enables a disabled server.
    pub fn seed_presets(&self) -> AppResult<()> {
        let args = playwright_v17_args(require_default_playwright_output_dir()?);
        for config in builtin_presets(PLAYWRIGHT_COMMAND, args, PLAYWRIGHT_DESCRIPTION) {
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

fn has_server(conn: &Connection, id: &str) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM mcp_servers WHERE id=?1)",
        [id],
        |row| row.get(0),
    )?)
}

fn require_manual_config(config: &ServerConfig) -> AppResult<()> {
    if config.plugin_slug.is_some() {
        return Err(AppError::Denied("不能通过手动配置设置插件 MCP 归属".into()));
    }
    Ok(())
}

fn manual_mutation_result(conn: &Connection, id: &str, count: usize) -> AppResult<()> {
    if count != 0 {
        Ok(())
    } else if has_server(conn, id)? {
        Err(AppError::Denied("不能手动修改或删除插件工具".into()))
    } else {
        Err(AppError::NotFound("MCP server".into()))
    }
}

fn inactive_plugin_error() -> AppError {
    AppError::Denied("所属插件必须已启用且通过验证才能启动工具".into())
}

fn insert_server(conn: &Connection, config: &ServerConfig, builtin: bool) -> AppResult<usize> {
    Ok(conn.execute(
        "INSERT INTO mcp_servers(id,name,transport,command,args_json,url,env_json,secret_ref,enabled,builtin,created_at,credential_url,plugin_slug,description)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
        params![config.id,config.name,config.transport.as_str(),config.command,serde_json::to_string(&config.args)?,config.url,
            serde_json::to_string(&config.env_secret_refs)?,config.bearer_secret_ref,config.enabled,builtin,now(),config.credential_url,config.plugin_slug,config.description],
    )?)
}

/// Sync only the given plugin's rows. The caller commits this together with its
/// plugin registry update. An empty list removes all MCP rows for this plugin.
/// Declared enablement is never trusted; unchanged rows keep their local state.
/// Returns whether any MCP rows changed in this transaction.
pub(crate) fn sync_plugin_servers(
    tx: &Transaction<'_>,
    slug: &str,
    configs: &[ServerConfig],
) -> AppResult<bool> {
    crate::plugins::validate_slug(slug)?;
    if configs.is_empty() {
        // Cleanup must not depend on successfully decoding any old launch config.
        return Ok(tx.execute("DELETE FROM mcp_servers WHERE plugin_slug=?1", [slug])? != 0);
    }
    let mut stmt = tx.prepare(&format!(
        "SELECT {SERVER_COLUMNS} FROM mcp_servers WHERE plugin_slug=?1"
    ))?;
    let existing: BTreeMap<_, _> = stmt
        .query_map([slug], from_row)?
        .map(|row| {
            row.map(|mut config| {
                // Compare manifest fields, not the local user's enablement choice.
                config.enabled = false;
                (config.id.clone(), config)
            })
        })
        .collect::<Result<_, _>>()?;

    // Preflight the whole batch before any write, especially before removing old rows.
    let mut desired = BTreeMap::new();
    for config in configs {
        config.validate()?;
        if config.builtin
            || config
                .plugin_slug
                .as_deref()
                .is_some_and(|owner| owner != slug)
        {
            return Err(AppError::Invalid("插件 MCP 配置的归属无效".into()));
        }
        if matches!(
            config.id.as_str(),
            crate::mcp_connections::LOCAL_SERVER_ID
                | GLOSSMOD_HTTP_ID
                | GLOSSMOD_3DM_ID
                | PLAYWRIGHT_ID
                | MARKITDOWN_ID
        ) || MOD_SOURCE_PRESETS
            .iter()
            .any(|preset| preset.id == config.id)
            || (!existing.contains_key(&config.id) && has_server(tx, &config.id)?)
        {
            return Err(AppError::Conflict(format!(
                "插件工具 ID 已被占用: {}",
                config.id
            )));
        }
        let mut config = config.clone();
        config.plugin_slug = Some(slug.into());
        config.enabled = false;
        if let Some(duplicate) = desired.insert(config.id.clone(), config) {
            return Err(AppError::Conflict(format!(
                "重复的插件工具 ID: {}",
                duplicate.id
            )));
        }
    }

    let mut changed = false;
    for (id, config) in &desired {
        if let Some(current) = existing.get(id) {
            if !config.same_connection_config(current) {
                changed |= tx.execute(
                    "UPDATE mcp_servers SET name=?2,transport=?3,command=?4,args_json=?5,url=?6,env_json=?7,secret_ref=?8,credential_url=?9,description=?10,enabled=0,startup_digest=NULL WHERE id=?1 AND plugin_slug=?11",
                    params![config.id,config.name,config.transport.as_str(),config.command,serde_json::to_string(&config.args)?,config.url,
                        serde_json::to_string(&config.env_secret_refs)?,config.bearer_secret_ref,config.credential_url,config.description,slug],
                )? != 0;
            } else if config.description != current.description {
                changed |= tx.execute(
                    "UPDATE mcp_servers SET description=?2 WHERE id=?1 AND plugin_slug=?3",
                    params![config.id, config.description, slug],
                )? != 0;
            }
        } else {
            changed |= insert_server(tx, config, false)? != 0;
        }
    }
    for id in existing.keys().filter(|id| !desired.contains_key(*id)) {
        changed |= tx.execute(
            "DELETE FROM mcp_servers WHERE id=?1 AND plugin_slug=?2",
            params![id, slug],
        )? != 0;
    }
    Ok(changed)
}

/// Revoke locally granted enablement and remembered launch without unregistering.
/// Returns whether any MCP rows changed in this transaction.
pub(crate) fn revoke_plugin_servers(tx: &Transaction<'_>, slug: &str) -> AppResult<bool> {
    Ok(tx.execute(
        "UPDATE mcp_servers SET enabled=0,startup_digest=NULL WHERE plugin_slug=?1
         AND (enabled<>0 OR startup_digest IS NOT NULL)",
        [slug],
    )? != 0)
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
        credential_url: row.get(10)?,
        plugin_slug: row.get(11)?,
        description: row.get(12)?,
    })
}

#[cfg(test)]
mod receipt_tests {
    use super::*;

    #[test]
    fn only_the_stock_launcher_and_independently_derived_output_path_are_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let expected = playwright_output_dir([dir.path().to_path_buf()]).unwrap();
        let args = playwright_v17_args(expected.clone());
        let mut stock = builtin_presets(PLAYWRIGHT_COMMAND, args, PLAYWRIGHT_DESCRIPTION)
            .find(|config| config.id == PLAYWRIGHT_ID)
            .unwrap();
        // Preset constructors are generic; only seeding marks the database row builtin.
        stock.builtin = true;
        assert_eq!(
            stock_playwright_receipt_dir(&stock, expected.clone()),
            Some(PathBuf::from(&expected))
        );
        let mut customized = stock.clone();
        customized.args =
            playwright_v17_args(dir.path().join("private").to_string_lossy().into_owned());
        assert_eq!(
            stock_playwright_receipt_dir(&customized, expected.clone()),
            None
        );

        let mut customized = stock.clone();
        customized.args.push("--output-dir=another".into());
        assert_eq!(
            stock_playwright_receipt_dir(&customized, expected.clone()),
            None
        );

        let mut customized = stock.clone();
        customized.command = Some("non-stock-launcher".into());
        assert_eq!(
            stock_playwright_receipt_dir(&customized, expected.clone()),
            None
        );

        let mut customized = stock.clone();
        customized.command = Some(LEGACY_PLAYWRIGHT_COMMAND.into());
        customized.args = playwright_v12_args(expected.clone());
        assert_eq!(
            stock_playwright_receipt_dir(&customized, expected.clone()),
            None
        );

        let mut customized = stock;
        customized
            .env_secret_refs
            .insert("HOME".into(), "mcp.home".into());
        assert_eq!(stock_playwright_receipt_dir(&customized, expected), None);
    }
}
