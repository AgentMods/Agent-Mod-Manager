//! Desktop browse capability for the seven stock, already-connected MCPs.
//! Keep arbitrary tool dispatch in the Agent approval path, not this module.
use super::{McpClient, McpServers, McpTool, ServerConfig};
use crate::error::{AppError, AppResult};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Source {
    GlossMod,
    ThreeDm,
    Nexus,
    CurseForge,
    GameBanana,
    ModIo,
    Thunderstore,
}

impl Source {
    pub(crate) const ALL: [Self; 7] = [
        Self::GlossMod,
        Self::ThreeDm,
        Self::ModIo,
        Self::CurseForge,
        Self::GameBanana,
        Self::Nexus,
        Self::Thunderstore,
    ];

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::GlossMod => "glossmod_http",
            Self::ThreeDm => "glossmod_3dm",
            Self::Nexus => "nexusmods",
            Self::CurseForge => "curseforge",
            Self::GameBanana => "gamebanana",
            Self::ModIo => "modio",
            Self::Thunderstore => "thunderstore",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Operation {
    Games,
    Browse,
    Search,
}

#[derive(Clone, Copy, PartialEq)]
enum Scalar {
    Str,
    Int,
    Bool,
}

struct Contract {
    name: &'static str,
    /// The query is the only optionally-supplied parameter. All others are fixed
    /// selectors/paging, or restrictive public/safety filters explicitly sent.
    fixed: &'static [(&'static str, Scalar)],
    query: Option<&'static str>,
    required_query: bool,
}

use Scalar::{Bool as B, Int as I, Str as S};

fn contract(source: Source, operation: Operation) -> Contract {
    use Operation::{Browse, Games, Search};
    use Source::*;
    let (name, fixed, query, required_query): (_, &[_], _, _) = match (source, operation) {
        (GlossMod, Games) => (
            "list_games",
            &[("offset", I), ("limit", I)],
            Some("q"),
            false,
        ),
        (GlossMod, Browse | Search) => (
            "list_works",
            &[("scope", S), ("game", S), ("offset", I), ("limit", I)],
            Some("keyword"),
            false,
        ),
        (ThreeDm, Games) => (
            "get-game-list",
            &[("page", I), ("pageSize", I)],
            Some("search"),
            false,
        ),
        (ThreeDm, Browse | Search) => (
            "get-mod-list",
            &[("gameId", I), ("page", I), ("pageSize", I)],
            Some("search"),
            false,
        ),
        (Nexus, Games) => (
            "find_games",
            &[("offset", I), ("count", I)],
            Some("name"),
            false,
        ),
        (Nexus, Browse | Search) => (
            "find_mods",
            &[
                ("game_domain", S),
                ("offset", I),
                ("count", I),
                ("adult_content", B),
            ],
            Some("name"),
            false,
        ),
        (CurseForge, Games) => (
            "list_games",
            &[("index", I), ("page_size", I)],
            Some("name"),
            false,
        ),
        (CurseForge, Browse | Search) => (
            "search_mods",
            &[("game_id", I), ("index", I), ("page_size", I)],
            Some("keyword"),
            false,
        ),
        (GameBanana, Games) => ("search_games", &[("page", I)], Some("query"), true),
        (GameBanana, Browse) => (
            "browse_mods",
            &[("game_id", I), ("page", I), ("per_page", I)],
            None,
            false,
        ),
        (GameBanana, Search) => (
            "search_mods",
            &[("game_id", I), ("page", I)],
            Some("query"),
            true,
        ),
        (ModIo, Games) => (
            "search_games",
            &[("offset", I), ("limit", I)],
            Some("query"),
            false,
        ),
        (ModIo, Browse | Search) => (
            "search_mods",
            &[("game_id", I), ("offset", I), ("limit", I)],
            Some("query"),
            false,
        ),
        (Thunderstore, Games) => (
            "list_communities",
            &[("offset", I), ("limit", I)],
            Some("search"),
            false,
        ),
        (Thunderstore, Browse | Search) => (
            "search_packages",
            &[("community", S), ("page", I), ("include_nsfw", B)],
            Some("query"),
            false,
        ),
    };
    Contract {
        name,
        fixed,
        query,
        required_query,
    }
}

impl Contract {
    fn schema_matches(&self, schema: &Value) -> bool {
        if schema.get("type").and_then(Value::as_str) != Some("object")
            || [
                "$ref",
                "allOf",
                "anyOf",
                "oneOf",
                "if",
                "not",
                "dependentRequired",
                "dependencies",
                "dependentSchemas",
            ]
            .iter()
            .any(|key| schema.get(key).is_some())
        {
            return false;
        }
        let Some(props) = schema.get("properties").and_then(Value::as_object) else {
            return false;
        };
        let valid_required = |key: &str| {
            self.fixed.iter().any(|(k, _)| *k == key)
                || (self.required_query && self.query == Some(key))
        };
        if let Some(required) = schema.get("required") {
            if !required.as_array().is_some_and(|keys| {
                keys.iter()
                    .all(|key| key.as_str().is_some_and(valid_required))
            }) {
                return false;
            }
        }
        self.fixed
            .iter()
            .copied()
            .chain(self.query.map(|key| (key, S)))
            .all(|(key, kind)| props.get(key).is_some_and(|v| supports(v, kind, None)))
    }

    fn arguments_match(&self, args: &Map<String, Value>, schema: &Value) -> bool {
        let expected = |key: &str| {
            self.fixed
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, kind)| *kind)
                .or_else(|| (self.query == Some(key)).then_some(S))
        };
        self.fixed.iter().all(|(key, _)| args.contains_key(*key))
            && (!self.required_query || self.query.is_some_and(|key| args.contains_key(key)))
            && args.iter().all(|(key, value)| {
                expected(key).is_some_and(|kind| {
                    let safe = match key.as_str() {
                        "scope" => value.as_str() == Some(""),
                        "adult_content" | "include_nsfw" => value.as_bool() == Some(false),
                        _ => true,
                    };
                    safe && supports(&schema["properties"][key], kind, Some(value))
                })
            })
    }
}

fn supports(schema: &Value, scalar: Scalar, value: Option<&Value>) -> bool {
    if !schema.is_object()
        || ["$ref", "allOf", "not", "if", "format"]
            .iter()
            .any(|k| schema.get(k).is_some())
    {
        return false;
    }
    if let Some(value) = value {
        let matches = match scalar {
            S => value.as_str().is_some_and(|s| {
                let len = s.chars().count() as u64;
                s.len() <= 512
                    && !s.chars().any(char::is_control)
                    && schema["minLength"].as_u64().is_none_or(|min| len >= min)
                    && schema["maxLength"].as_u64().is_none_or(|max| len <= max)
                    && schema["pattern"].as_str().is_none_or(|p| {
                        p.len() <= 256 && regex::Regex::new(p).is_ok_and(|rx| rx.is_match(s))
                    })
            }),
            I => value.as_u64().is_some_and(|n| {
                let n = n as f64;
                schema["minimum"].as_f64().is_none_or(|min| n >= min)
                    && schema["maximum"].as_f64().is_none_or(|max| n <= max)
                    && schema["exclusiveMinimum"]
                        .as_f64()
                        .is_none_or(|min| n > min)
                    && schema["exclusiveMaximum"]
                        .as_f64()
                        .is_none_or(|max| n < max)
            }),
            B => value.is_boolean(),
        };
        if !matches
            || schema.get("const").is_some_and(|v| v != value)
            || schema
                .get("enum")
                .is_some_and(|e| !e.as_array().is_some_and(|v| v.contains(value)))
        {
            return false;
        }
    }
    // Stock FastMCP uses anyOf for nullables; 3DM also uses anyOf for
    // single game ID vs an array. We only ever send one typed scalar.
    let type_name = match scalar {
        S => "string",
        I => "integer",
        B => "boolean",
    };
    if let Some(branches) = schema.get("anyOf") {
        return schema.get("type").is_none()
            && schema.get("oneOf").is_none()
            && branches
                .as_array()
                .is_some_and(|b| b.iter().any(|v| supports(v, scalar, value)));
    }
    schema.get("oneOf").is_none() && schema["type"].as_str() == Some(type_name)
}

/// Not serializable and not based on secret values. Reusing an identical config
/// after a reconnect still produces a different identity.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct BrowseConnectionId(uuid::Uuid);

impl BrowseConnectionId {
    #[cfg(test)]
    pub(crate) fn fixture() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

#[derive(Clone)]
pub(crate) struct BrowseReader {
    client: McpClient,
    source: Source,
    id: BrowseConnectionId,
    schemas: Arc<[Value; 3]>,
}

impl BrowseReader {
    pub(crate) fn from_discovery(client: &McpClient, tools: &[McpTool]) -> Option<Self> {
        let source = Source::from_id(client.server_id())?;
        if !client.server_config().is_stock_browse_source() {
            return None;
        }
        let schema = |operation| {
            let contract = contract(source, operation);
            tools
                .iter()
                .find(|tool| {
                    tool.remote_name == contract.name && contract.schema_matches(&tool.input_schema)
                })
                .map(|tool| tool.input_schema.clone())
        };
        Some(Self {
            client: client.clone(),
            source,
            id: BrowseConnectionId(uuid::Uuid::new_v4()),
            schemas: Arc::new([
                schema(Operation::Games)?,
                schema(Operation::Browse)?,
                schema(Operation::Search)?,
            ]),
        })
    }

    pub(crate) fn identity(&self) -> BrowseConnectionId {
        self.id.clone()
    }

    pub(crate) async fn read(
        &self,
        operation: Operation,
        args: Value,
        cancel: &CancellationToken,
    ) -> AppResult<Value> {
        let contract = contract(self.source, operation);
        let schema = &self.schemas[operation as usize];
        if !args
            .as_object()
            .is_some_and(|args| contract.arguments_match(args, schema))
        {
            return Err(AppError::Invalid(
                "游览查询不符合已发现的 MCP 参数契约".into(),
            ));
        }
        let response = self
            .client
            .call_browse_tool(contract.name, args, cancel)
            .await?;
        self.client.redact_browse_catalog(decode(response)?)
    }
}

fn decode(mut response: Value) -> AppResult<Value> {
    let invalid = || AppError::Other("MCP 返回的公开目录不符合契约".into());
    let parse_text = |text: &str| serde_json::from_str::<Value>(text).ok();
    if response["isError"].as_bool().unwrap_or(false) {
        // Recognize typed server authorization errors and fixed status prefixes in
        // stock package errors. Never return the remote message itself.
        let has_denied_code =
            |val: &Value| matches!(val["code"].as_i64(), Some(401 | 403 | -32001 | -32003));
        let denied = has_denied_code(&response["structuredContent"])
            || response["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v["text"].as_str())
                .any(|text| {
                    parse_text(text).as_ref().is_some_and(has_denied_code)
                        || [
                            "HTTP 401",
                            "HTTP 403",
                            "CurseForge HTTP 401",
                            "CurseForge HTTP 403",
                            "mod.io HTTP 401",
                            "mod.io HTTP 403",
                        ]
                        .iter()
                        .any(|token| text.contains(token))
                });
        return Err(if denied {
            AppError::Denied("MCP 目录读取权限不足".into())
        } else {
            invalid()
        });
    }
    if let Some(value) = response.get_mut("structuredContent") {
        if value.is_object() {
            return Ok(value.take());
        }
        return Err(invalid());
    }
    // Other content types, embedded resources, and multiple textual payloads
    // are not a catalog. Never follow their URLs or let them supply instructions.
    let content = response["content"]
        .as_array()
        .filter(|items| items.len() == 1)
        .ok_or_else(invalid)?;
    let text = content[0]
        .get("text")
        .filter(|_| content[0]["type"] == "text")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    parse_text(text)
        .filter(Value::is_object)
        .ok_or_else(invalid)
}

/// Per-reference monotonic metadata, changed even if two writes use the same
/// millisecond `updated_at`. This SQLite connection only observes ref lifecycle;
/// no encrypted store access nor hash of the credential is involved.
#[derive(Clone, Eq, PartialEq)]
pub(super) struct CredentialRevision(Vec<(bool, i64)>);

pub(super) fn install_credential_observer(conn: &Connection) -> AppResult<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS mcp_browse_credential_revision (
        key TEXT PRIMARY KEY, revision INTEGER NOT NULL CHECK (revision >= 1)
      );
      CREATE TRIGGER IF NOT EXISTS mcp_browse_credential_insert AFTER INSERT ON secrets_ref
      WHEN NEW.key GLOB 'mcp.*' BEGIN
        INSERT INTO mcp_browse_credential_revision VALUES(NEW.key, 1)
        ON CONFLICT(key) DO UPDATE SET revision=revision+1;
      END;
      CREATE TRIGGER IF NOT EXISTS mcp_browse_credential_update AFTER UPDATE ON secrets_ref
      WHEN NEW.key GLOB 'mcp.*' OR OLD.key GLOB 'mcp.*' BEGIN
        INSERT INTO mcp_browse_credential_revision SELECT OLD.key, 1 WHERE OLD.key GLOB 'mcp.*'
        ON CONFLICT(key) DO UPDATE SET revision=revision+1;
        INSERT INTO mcp_browse_credential_revision SELECT NEW.key, 1 WHERE NEW.key GLOB 'mcp.*' AND NEW.key<>OLD.key
        ON CONFLICT(key) DO UPDATE SET revision=revision+1;
      END;
      CREATE TRIGGER IF NOT EXISTS mcp_browse_credential_delete AFTER DELETE ON secrets_ref
      WHEN OLD.key GLOB 'mcp.*' BEGIN
        INSERT INTO mcp_browse_credential_revision VALUES(OLD.key, 1)
        ON CONFLICT(key) DO UPDATE SET revision=revision+1;
      END;")?;
    Ok(())
}

pub(super) fn credential_revision(
    servers: &McpServers,
    config: &ServerConfig,
    initialize: bool,
) -> AppResult<CredentialRevision> {
    let keys: Vec<_> = config
        .bearer_secret_ref
        .iter()
        .chain(config.env_secret_refs.values())
        .collect();
    if keys.is_empty() {
        return Ok(CredentialRevision(vec![]));
    }
    let conn = servers.db.conn();
    if initialize {
        install_credential_observer(&conn)?;
    }
    let revisions = keys
        .into_iter()
        .map(|key| {
            let present = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM secrets_ref WHERE key=?1)",
                [key],
                |r| r.get::<_, bool>(0),
            )?;
            let revision = conn
                .query_row(
                    "SELECT revision FROM mcp_browse_credential_revision WHERE key=?1",
                    [key],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0);
            Ok((present, revision))
        })
        .collect::<AppResult<_>>()?;
    Ok(CredentialRevision(revisions))
}

pub(super) fn public_modio_base(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value.trim()) else {
        return false;
    };
    url.scheme() == "https"
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && matches!(url.path(), "/v1" | "/v1/")
        && url.host_str().is_some_and(|host| {
            host == "api.mod.io"
                || host
                    .strip_suffix(".modapi.io")
                    .and_then(|prefix| {
                        prefix
                            .strip_prefix("u-")
                            .or_else(|| prefix.strip_prefix("g-"))
                    })
                    .is_some_and(|number| {
                        !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
                    })
        })
}

#[cfg(test)]
mod tests;
