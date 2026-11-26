//! Normalize seven distinct, explicitly scoped public MCP catalogues. No raw
//! MCP payloads (or links discovered in non-public metadata) leave this layer.
use crate::{
    error::{AppError, AppResult},
    mcp::{
        browse::{BrowseConnectionId, BrowseReader, Operation, Source},
        McpServers,
    },
    mcp_connections::McpConnections,
};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use url::Url;

pub(crate) type ProviderConnection = BrowseConnectionId;

#[cfg(test)]
pub(super) fn test_connection() -> ProviderConnection {
    BrowseConnectionId::fixture()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderAvailability {
    Available,
    NotConnected,
    PermissionDenied,
    Unsupported,
}

pub(crate) struct ProviderSource {
    pub id: &'static str,
    pub connection: Option<ProviderConnection>,
    pub availability: ProviderAvailability,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProviderItem {
    pub id: String,
    pub title: String,
    pub author: Option<String>,
    pub summary: Option<String>,
    pub cover_url: Option<String>,
    pub page_url: Option<String>,
    pub kind: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProviderPage {
    pub items: Vec<ProviderItem>,
    pub next: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProviderGame {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProviderGamePage {
    pub games: Vec<ProviderGame>,
    pub next: Option<String>,
}

pub(crate) fn sources(connections: &McpConnections, servers: &McpServers) -> Vec<ProviderSource> {
    Source::ALL
        .into_iter()
        .map(|source| {
            let identity = connection(connections, servers, source.id());
            let availability = match &identity {
                Ok(_) => ProviderAvailability::Available,
                Err(AppError::Denied(_)) => ProviderAvailability::PermissionDenied,
                Err(AppError::NotFound(_) | AppError::Conflict(_)) => {
                    ProviderAvailability::NotConnected
                }
                Err(_) => ProviderAvailability::Unsupported,
            };
            ProviderSource {
                id: source.id(),
                connection: identity.ok(),
                availability,
            }
        })
        .collect()
}

pub(crate) fn connection(
    connections: &McpConnections,
    servers: &McpServers,
    source: &str,
) -> AppResult<ProviderConnection> {
    require_source(source)?;
    Ok(connections.browse_reader(servers, source)?.identity())
}

fn require_source(source: &str) -> AppResult<Source> {
    Source::from_id(source).ok_or_else(|| AppError::Unsupported("不是可浏览的原厂 MCP 来源".into()))
}

fn bound_reader(
    connections: &McpConnections,
    servers: &McpServers,
    source: Source,
    expected: &ProviderConnection,
    cancel: &CancellationToken,
) -> AppResult<BrowseReader> {
    check_cancelled(cancel)?;
    let reader = connections.browse_reader(servers, source.id())?;
    if &reader.identity() != expected {
        return Err(AppError::Conflict(
            "MCP 连接身份已变化，请重新确认游览".into(),
        ));
    }
    Ok(reader)
}

fn check_cancelled(cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}

pub(crate) async fn list_games(
    connections: &McpConnections,
    servers: &McpServers,
    source: &str,
    expected: &ProviderConnection,
    query: &str,
    cursor: Option<&str>,
    cancel: &CancellationToken,
) -> AppResult<ProviderGamePage> {
    let source = require_source(source)?;
    let reader = bound_reader(connections, servers, source, expected, cancel)?;
    let request = Request::new(source, Operation::Games, None, query, cursor)?;
    let value = reader
        .read(request.operation, request.args.clone(), cancel)
        .await?;
    let (rows, next) = request.rows(&value)?;
    let games = rows
        .iter()
        .filter_map(|row| parse_game(source, row, query))
        .collect();
    bound_reader(connections, servers, source, expected, cancel)?;
    Ok(ProviderGamePage { games, next })
}

#[allow(clippy::too_many_arguments)] // Each query carries a current authorization and a distinct game/query/cursor.
pub(crate) async fn search(
    connections: &McpConnections,
    servers: &McpServers,
    source: &str,
    expected: &ProviderConnection,
    game_id: Option<&str>,
    query: &str,
    cursor: Option<&str>,
    cancel: &CancellationToken,
) -> AppResult<ProviderPage> {
    let source = require_source(source)?;
    let reader = bound_reader(connections, servers, source, expected, cancel)?;
    let operation = if query.trim().is_empty() {
        Operation::Browse
    } else {
        Operation::Search
    };
    let request = Request::new(source, operation, game_id, query, cursor)?;
    let value = reader
        .read(request.operation, request.args.clone(), cancel)
        .await?;
    let (rows, next) = request.rows(&value)?;
    let game = game_id.ok_or_else(invalid_args)?;
    let items = rows
        .iter()
        .filter_map(|row| parse_item(source, row, game))
        .collect();
    bound_reader(connections, servers, source, expected, cancel)?;
    Ok(ProviderPage { items, next })
}

const PAGE_SIZE: u64 = 20;
const MAX_POSITION: u64 = 9_999;
// The 3DM tools advertise a larger integer range but reject page 1001.
const THREE_DM_MAX_PAGE: u64 = 1_000;

fn invalid_args() -> AppError {
    AppError::Invalid("MCP 公开目录查询参数无效".into())
}
fn invalid_page() -> AppError {
    AppError::Other("MCP 公开目录分页不符合契约".into())
}

fn max_position(source: Source) -> u64 {
    if source == Source::ThreeDm {
        THREE_DM_MAX_PAGE
    } else {
        MAX_POSITION
    }
}

struct Request {
    source: Source,
    operation: Operation,
    position: u64,
    size: u64,
    args: Value,
}

impl Request {
    fn new(
        source: Source,
        operation: Operation,
        game_id: Option<&str>,
        query: &str,
        cursor: Option<&str>,
    ) -> AppResult<Self> {
        use Source::*;
        let games = matches!(operation, Operation::Games);
        let query = query.trim();
        if query.len() > 512
            || query.chars().any(char::is_control)
            || (source == ThreeDm && query.chars().count() > 100)
        {
            return Err(invalid_args());
        }
        let page_indexed =
            matches!(source, ThreeDm | GameBanana) || source == Thunderstore && !games;
        let start = u64::from(page_indexed);
        let position = match cursor {
            None => start,
            Some(c) => number_string(c)
                .or_else(|| (c == "0").then_some(0))
                .filter(|n| (start..=max_position(source)).contains(n))
                .ok_or_else(invalid_args)?,
        };
        let size = if source == CurseForge {
            (10_000 - position).min(if games { 50 } else { PAGE_SIZE })
        } else {
            PAGE_SIZE
        };
        let game = |numeric| {
            let id = game_id.ok_or_else(invalid_args)?;
            if numeric {
                number_string(id)
                    .map(|id| json!(id))
                    .ok_or_else(invalid_args)
            } else {
                slug(id).map(|id| json!(id)).ok_or_else(invalid_args)
            }
        };
        let (mut args, query_key) = match (source, games) {
            (GlossMod, true) => (json!({"offset":position,"limit":size}), "q"),
            (GlossMod, false) => (
                json!({"scope":"", "game":game(false)?,"offset":position,"limit":size}),
                "keyword",
            ),
            (ThreeDm, true) => (json!({"page":position,"pageSize":size}), "search"),
            (ThreeDm, false) => (
                json!({"gameId":game(true)?,"page":position,"pageSize":size}),
                "search",
            ),
            (Nexus, true) => (json!({"offset":position,"count":size}), "name"),
            (Nexus, false) => (
                json!({"game_domain":game(false)?,"offset":position,"count":size,"adult_content":false}),
                "name",
            ),
            (CurseForge, true) => (json!({"index":position,"page_size":size}), "name"),
            (CurseForge, false) => (
                json!({"game_id":game(true)?,"index":position,"page_size":size}),
                "keyword",
            ),
            (GameBanana, true) if query.is_empty() => return Err(invalid_args()),
            (GameBanana, true) => (json!({"page":position}), "query"),
            (GameBanana, false) if matches!(operation, Operation::Browse) => (
                json!({"game_id":game(true)?,"page":position,"per_page":size}),
                "",
            ),
            (GameBanana, false) => (json!({"game_id":game(true)?,"page":position}), "query"),
            (ModIo, true) => (json!({"offset":position,"limit":size}), "query"),
            (ModIo, false) => (
                json!({"game_id":game(true)?,"offset":position,"limit":size}),
                "query",
            ),
            (Thunderstore, true) => (json!({"offset":position,"limit":size}), "search"),
            (Thunderstore, false) => (
                json!({"community":game(false)?,"page":position,"include_nsfw":false}),
                "query",
            ),
        };
        if !query.is_empty() && !query_key.is_empty() {
            args[query_key] = json!(query);
        }
        Ok(Self {
            source,
            operation,
            position,
            size,
            args,
        })
    }

    fn rows<'a>(&self, payload: &'a Value) -> AppResult<(&'a [Value], Option<String>)> {
        use Source::*;
        let games = matches!(self.operation, Operation::Games);
        let payload = if self.source == ThreeDm {
            if payload["success"] != true {
                return Err(invalid_page());
            }
            &payload["data"]
        } else {
            payload
        };
        let list = match self.source {
            GlossMod => "items",
            Nexus => "nodes",
            GameBanana => "_aRecords",
            Thunderstore => "results",
            _ => "data",
        };
        let rows = payload[list]
            .as_array()
            .filter(|rows| rows.len() <= 50)
            .ok_or_else(invalid_page)?;
        let len = rows.len() as u64;
        let next = match self.source {
            // No total/next is promised by this exact public contract. A full
            // page has a possible continuation; a short page terminates.
            GlossMod => (len == self.size).then_some(self.position + len),
            ThreeDm => {
                if payload["page"].as_u64() != Some(self.position) {
                    return Err(invalid_page());
                }
                let total = payload["totalPages"].as_u64().ok_or_else(invalid_page)?;
                if len > self.size
                    || (len != 0 && self.position > total)
                    || payload
                        .get("pageSize")
                        .is_some_and(|size| size.as_u64() != Some(self.size))
                {
                    return Err(invalid_page());
                }
                (self.position < total && len != 0).then_some(self.position + 1)
            }
            Nexus => {
                let total = payload["totalCount"].as_u64().ok_or_else(invalid_page)?;
                (len != 0 && self.position + len < total).then_some(self.position + len)
            }
            // The package filters games AFTER fetching an upstream page, so
            // an empty results array may legitimately carry nextIndex.
            CurseForge => optional_position(payload, "nextIndex")?,
            ModIo => optional_position(payload, "next_offset")?,
            GameBanana => {
                let complete = payload["_aMetadata"]["_bIsComplete"]
                    .as_bool()
                    .ok_or_else(invalid_page)?;
                (!complete).then_some(self.position + 1)
            }
            Thunderstore if games => optional_position(payload, "next_offset")?,
            // Only consume the presence of a continuation. Never follow or
            // forward the upstream URL (which can carry arbitrary tokens).
            Thunderstore => match payload.get("next") {
                Some(Value::String(next)) if !next.is_empty() && len != 0 => {
                    Some(self.position + 1)
                }
                None | Some(Value::Null) => None,
                _ => return Err(invalid_page()),
            },
        };
        if next.is_some_and(|n| n <= self.position) {
            return Err(invalid_page());
        }
        Ok((
            rows,
            next.filter(|n| *n <= max_position(self.source))
                .map(|n| n.to_string()),
        ))
    }
}

fn optional_position(v: &Value, key: &str) -> AppResult<Option<u64>> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(n) => n.as_u64().map(Some).ok_or_else(invalid_page),
    }
}

fn number_string(s: &str) -> Option<u64> {
    s.parse::<u64>()
        .ok()
        .filter(|n| (1..=9_007_199_254_740_991).contains(n) && n.to_string() == s)
}

fn number(v: &Value) -> Option<u64> {
    v.as_u64()
        .filter(|n| (1..=9_007_199_254_740_991).contains(n))
        .or_else(|| number_string(v.as_str()?))
}

fn slug(s: &str) -> Option<&str> {
    (!s.is_empty()
        && s.len() <= 75
        && s.as_bytes().iter().any(u8::is_ascii_alphanumeric)
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')))
    .then_some(s)
}

fn text(v: &Value, max: usize) -> Option<String> {
    let mut out = String::new();
    for c in v.as_str()?.chars() {
        let c = match c {
            '\n' | '\r' | '\t' => ' ',
            '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' => continue,
            c if c.is_control() => continue,
            c => c,
        };
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    let out = out.trim();
    (!out.is_empty()).then(|| out.to_owned())
}

fn tags(value: &Value, name: Option<&str>) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .take(24)
        .filter_map(|v| text(name.map_or(v, |key| &v[key]), 80))
        .collect()
}

fn parse_game(source: Source, v: &Value, query: &str) -> Option<ProviderGame> {
    use Source::*;
    let (id, label) = match source {
        GlossMod => {
            if v["active"] == false {
                return None;
            }
            let label = v["aliases"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|a| {
                    a.as_str()
                        .is_some_and(|name| name.eq_ignore_ascii_case(query.trim()))
                })
                .unwrap_or(&v["name"]);
            (slug(v["slug"].as_str()?)?.to_owned(), label)
        }
        ThreeDm => {
            let label = if v["game_ename"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(query.trim()))
            {
                &v["game_ename"]
            } else {
                &v["game_name"]
            };
            (number(&v["id"])?.to_string(), label)
        }
        Nexus => (slug(v["domainName"].as_str()?)?.to_owned(), &v["name"]),
        GameBanana => (number(&v["_idRow"])?.to_string(), &v["_sName"]),
        ModIo if v.get("status").is_some_and(|v| number(v) != Some(1)) => return None,
        CurseForge | ModIo => (number(&v["id"])?.to_string(), &v["name"]),
        Thunderstore if v["is_listed"] == false => return None,
        Thunderstore => (slug(v["identifier"].as_str()?)?.to_owned(), &v["name"]),
    };
    Some(ProviderGame {
        id,
        name: text(label, 180)?,
    })
}

fn explicit_game_matches(row: &Value, field: &str, expected: &str) -> bool {
    row.get(field)
        .is_none_or(|id| number(id) == number_string(expected))
}

fn publicly_visible(row: &Value, key: &str) -> bool {
    row.get(key)
        .is_none_or(|value| value == true || number(value) == Some(1))
}

fn parse_item(source: Source, v: &Value, game: &str) -> Option<ProviderItem> {
    use Source::*;
    let (id, title, author, summary, cover, page_url, kind, tags) = match source {
        GlossMod => {
            if v["status"] != "published" {
                return None;
            }
            let id = number(&v["id"])?;
            (
                id.to_string(),
                &v["title"],
                text(&v["creator_name"], 180),
                &v["summary"],
                cover(source, v["cover_url"].as_str()),
                Some(format!("https://www.glossmod.com/mods/{id}")),
                text(&v["type"], 100),
                tags(&v["tags"], None),
            )
        }
        ThreeDm => {
            if number(&v["game_id"]) != Some(number_string(game)?) {
                return None;
            }
            let id = number(&v["id"])?;
            (
                id.to_string(),
                &v["mods_title"],
                text(&v["mods_author"], 180),
                &v["mods_desc"],
                cover(source, v["mods_image_url"].as_str()),
                Some(format!("https://mod.3dmgame.com/mod/{id}")),
                text(&v["mods_type_name"], 100),
                tags(&v["mods_key"], None),
            )
        }
        Nexus => {
            // Nexus's numeric modId is scoped to a game domain, not globally unique.
            let domain = slug(v["game"]["domainName"].as_str()?)?;
            if domain != game || v["adultContent"] == true {
                return None;
            }
            let id = number(&v["modId"])?;
            (
                format!("{domain}/{id}"),
                &v["name"],
                text(&v["author"], 180),
                &v["summary"],
                cover(source, v["pictureUrl"].as_str()),
                Some(format!("https://www.nexusmods.com/{domain}/mods/{id}")),
                text(&v["modCategory"]["name"], 100),
                tags(&v["tags"], Some("name")),
            )
        }
        CurseForge => {
            if !explicit_game_matches(v, "gameId", game) {
                return None;
            }
            let id = number(&v["id"])?;
            let categories = tags(&v["categories"], Some("name"));
            let authors = text(&v["authors"][0]["name"], 180);
            let page_url = strict_url(v["links"]["websiteUrl"].as_str()).and_then(|url| {
                matches!(url.host_str()?, "www.curseforge.com" | "curseforge.com").then_some(())?;
                let parts = path_parts(&url)?;
                let [site_game, listing, project] = parts.as_slice() else {
                    return None;
                };
                slug(site_game)?;
                if !matches!(
                    *listing,
                    "mc-mods"
                        | "mods"
                        | "addons"
                        | "texture-packs"
                        | "resource-packs"
                        | "modpacks"
                        | "worlds"
                        | "data-packs"
                        | "shaders"
                        | "customization"
                        | "plugins"
                        | "bukkit-plugins"
                ) {
                    return None;
                }
                if slug(project)? != v["slug"].as_str()? {
                    return None;
                }
                Some(url.into())
            });
            (
                id.to_string(),
                &v["name"],
                authors,
                &v["summary"],
                cover(source, v["logo"]["url"].as_str()),
                page_url,
                categories.first().cloned(),
                categories,
            )
        }
        GameBanana => {
            if !explicit_game_matches(v, "_idGameRow", game)
                || !explicit_game_matches(&v["_aGame"], "_idRow", game)
                || v.get("_sModelName").is_some_and(|value| value != "Mod")
            {
                return None;
            }
            let id = number(&v["_idRow"])?;
            let img = &v["_aPreviewMedia"]["_aImages"][0];
            let thumbnail = img["_sBaseUrl"].as_str().and_then(|base| {
                let file = img["_sFile220"]
                    .as_str()
                    .or_else(|| img["_sFile"].as_str())?;
                (!file.contains('/') && !file.contains('\\'))
                    .then(|| format!("{}/{file}", base.trim_end_matches('/')))
            });
            (
                id.to_string(),
                &v["_sName"],
                text(&v["_aSubmitter"]["_sName"], 180),
                &v["_sDescription"],
                cover(source, thumbnail.as_deref()),
                Some(format!("https://gamebanana.com/mods/{id}")),
                text(&v["_aRootCategory"]["_sName"], 100)
                    .or_else(|| text(&v["_aCategory"]["_sName"], 100)),
                vec![],
            )
        }
        ModIo => {
            if !explicit_game_matches(v, "game_id", game)
                || !publicly_visible(v, "status")
                || !publicly_visible(v, "visible")
            {
                return None;
            }
            let id = number(&v["id"])?;
            let page_url = strict_url(v["profile_url"].as_str()).and_then(|url| {
                matches!(url.host_str()?, "mod.io" | "www.mod.io").then_some(())?;
                let parts = path_parts(&url)?;
                let ["g", site_game, "m", project] = parts.as_slice() else {
                    return None;
                };
                slug(site_game)?;
                if slug(project)? != v["name_id"].as_str()? {
                    return None;
                }
                Some(url.into())
            });
            (
                id.to_string(),
                &v["name"],
                text(&v["submitted_by"]["username"], 180),
                &v["summary"],
                cover(
                    source,
                    v["logo"]["thumb_320x180"]
                        .as_str()
                        .or_else(|| v["logo"]["original"].as_str()),
                ),
                page_url,
                None,
                tags(&v["tags"], Some("name")),
            )
        }
        Thunderstore => {
            if v["is_nsfw"] == true
                || v.get("community_identifier")
                    .is_some_and(|id| id.as_str() != Some(game))
            {
                return None;
            }
            let owner = slug(v["namespace"].as_str()?)?;
            let name = slug(v["name"].as_str()?)?;
            let categories = tags(&v["categories"], Some("name"));
            (
                format!("{owner}/{name}"),
                &v["name"],
                Some(owner.to_owned()),
                &v["description"],
                cover(source, v["icon_url"].as_str()),
                Some(format!(
                    "https://thunderstore.io/c/{game}/p/{owner}/{name}/"
                )),
                categories.first().cloned(),
                categories,
            )
        }
    };
    Some(ProviderItem {
        id,
        title: text(title, 260)?,
        author,
        summary: text(summary, 1000),
        cover_url: cover,
        page_url,
        kind,
        tags,
    })
}

fn strict_url(raw: Option<&str>) -> Option<Url> {
    let raw = raw?;
    let rest = raw.strip_prefix("https://")?;
    if raw.len() > 1024
        || raw.contains("[REDACTED]")
        || raw
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | '%' | '?' | '#'))
        || rest.split('/').next()?.contains('@')
        || rest
            .split('/')
            .skip(1)
            .any(|segment| matches!(segment, "." | ".."))
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    (url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.host_str().is_some()
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

fn path_parts(url: &Url) -> Option<Vec<&str>> {
    let parts: Vec<_> = url
        .path()
        .trim_start_matches('/')
        .trim_end_matches('/')
        .split('/')
        .collect();
    parts.iter().all(|p| !p.is_empty()).then_some(parts)
}

pub(super) fn cover(source: Source, raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    let relative;
    let three_dm_path = |path: &str| {
        ["/static/upload/logo/", "/static/upload/mod/"]
            .iter()
            .any(|prefix| path.starts_with(prefix))
    };
    let url = if source == Source::ThreeDm && three_dm_path(raw) {
        relative = format!("https://mod.3dmgame.com{raw}");
        strict_url(Some(&relative))?
    } else {
        strict_url(Some(raw))?
    };
    let host = url.host_str()?;
    let valid = match source {
        Source::GlossMod => {
            matches!(host, "r2.glossmod.com" | "cdn.glossmod.com")
                && (url.path().starts_with("/media/") || url.path().starts_with("/works/"))
        }
        Source::ThreeDm => {
            matches!(
                host,
                "mod.3dmgame.com" | "img.3dmgame.com" | "dl.3dmgame.com" | "dmod.3dmgame.com"
            ) && three_dm_path(url.path())
        }
        Source::Nexus => matches!(
            host,
            "staticdelivery.nexusmods.com"
                | "images.nexusmods.com"
                | "staticdelivery.nexus-cdn.com"
        ),
        Source::CurseForge => matches!(host, "media.forgecdn.net" | "mediafilez.forgecdn.net"),
        Source::GameBanana => matches!(host, "images.gamebanana.com" | "files.gamebanana.com"),
        Source::ModIo => host.ends_with(".modcdn.io"),
        Source::Thunderstore => matches!(
            host,
            "ccdn.thunderstore.io" | "gcdn.thunderstore.io" | "cdn.thunderstore.io"
        ),
    };
    let is_image = url.path().rsplit_once('.').is_some_and(|(_, ext)| {
        ["jpg", "jpeg", "png", "webp", "avif", "gif"]
            .iter()
            .any(|e| ext.eq_ignore_ascii_case(e))
    });
    // 3DM also serves WebP variants with an @webp suffix, including names
    // without a conventional extension. Only permit these on the paths above.
    let three_dm_webp = source == Source::ThreeDm
        && url.path().rsplit('/').next().is_some_and(|file| {
            file.strip_suffix("@webp").is_some_and(|name| {
                !name.is_empty()
                    && name.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')
                    })
            })
        });
    (valid && (is_image || three_dm_webp)).then(|| url.into())
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod three_dm_tests;
