use super::{AppError, AppResult, ModSummary, Provider, SearchQuery, SearchResult, SourceState};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;

const ENDPOINT: &str = "https://api.nexusmods.com/v2/graphql";

// Fixed read-only document; user input only enters JSON variables.
// https://graphql.nexusmods.com/
const FIND_MODS: &str = r#"
query FindMods($game: String!, $term: String!, $count: Int!, $offset: Int!) {
  mods(
    viewUploaderHidden: false
    filter: { op: AND, filter: [
      { gameDomainName: { value: $game, op: EQUALS } }
      { nameStemmed: { value: $term, op: MATCHES } }
    ] }
    sort: [{ relevance: { direction: DESC } }]
    count: $count
    offset: $offset
  ) {
    totalCount
    nodes { modId name summary downloads author game { domainName } }
  }
}
"#;

#[derive(Deserialize)]
struct GraphqlResponse {
    data: Option<SearchData>,
    #[serde(default)]
    errors: Vec<serde::de::IgnoredAny>,
}

#[derive(Deserialize)]
struct SearchData {
    mods: NexusPage,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NexusPage {
    total_count: u64,
    nodes: Vec<NexusProject>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NexusProject {
    mod_id: u64,
    name: String,
    summary: Option<String>,
    author: Option<String>,
    downloads: Option<u64>,
    game: NexusGame,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NexusGame {
    domain_name: String,
}

impl Provider {
    pub(super) async fn nexus_text_search(
        &self,
        game: &str,
        term: &str,
        query: &SearchQuery,
    ) -> AppResult<SearchResult> {
        if query.game_version.is_some() || query.loader.is_some() {
            return Ok(self.empty(
                SourceState::Unsupported,
                "Nexus 搜索不支持游戏版本或加载器筛选；请清除筛选后按名称搜索",
            ));
        }
        let request = self.authenticate(self.client.post(ENDPOINT)).json(&json!({
            "query": FIND_MODS,
            "variables": { "game": game, "term": term, "count": query.limit, "offset": query.offset },
        }));
        // Shared origin admission, TLS and bounded JSON apply. POST never
        // enters the GET response cache, and there is no endpoint fallback.
        let response: Value = self.fetch_json(request).await?;
        let parsed: GraphqlResponse =
            serde_json::from_value(response).map_err(|_| invalid_page())?;
        if !parsed.errors.is_empty() {
            // A GraphQL HTTP 200 may include partial data and arbitrary error
            // strings. Neither should be treated as a successful search.
            return Err(AppError::Other("Nexus 搜索接口拒绝或未完成查询".into()));
        }
        let NexusPage { total_count, nodes } = parsed.data.ok_or_else(invalid_page)?.mods;
        let count = nodes.len() as u64;
        let end = u64::from(query.offset) + count;
        if count > u64::from(query.limit)
            || (count != 0 && end > total_count)
            || (count == 0 && end < total_count)
        {
            return Err(invalid_page());
        }
        let mut seen = HashSet::with_capacity(nodes.len());
        let mut mods = Vec::with_capacity(nodes.len());
        for node in nodes {
            if node.game.domain_name != game
                || node.mod_id == 0
                || !seen.insert(node.mod_id)
                || node.name.trim().is_empty()
                || !valid_display_text(&node.name, 512, false)
                || node
                    .summary
                    .as_ref()
                    .is_some_and(|s| !valid_display_text(s, 2048, true))
                || node
                    .author
                    .as_ref()
                    .is_some_and(|s| !valid_display_text(s, 512, false))
            {
                return Err(invalid_page());
            }
            let id = node.mod_id;
            mods.push(ModSummary {
                source_id: self.id.into(),
                project_id: format!("{game}/{id}"),
                game_id: Some(game.into()),
                name: node.name,
                summary: node.summary.unwrap_or_default(),
                author: node.author.filter(|author| !author.trim().is_empty()),
                homepage_url: Some(format!("https://www.nexusmods.com/{game}/mods/{id}")),
                icon_url: None,
                downloads: node.downloads,
            });
        }
        let status = if end < total_count {
            "已按游戏搜索 Mod 名称；还有更多结果。直接下载受账户权限限制"
        } else {
            "已按游戏搜索 Mod 名称；直接下载受账户权限限制"
        };
        Ok(SearchResult {
            mods,
            sources: vec![self.state(SourceState::Limited, status)],
        })
    }
}

fn valid_display_text(text: &str, max_bytes: usize, multiline: bool) -> bool {
    text.len() <= max_bytes
        && !text
            .chars()
            .any(|c| c.is_control() && !(multiline && matches!(c, '\n' | '\r' | '\t')))
}

fn invalid_page() -> AppError {
    AppError::Other("Nexus 搜索结果格式或游戏范围无效".into())
}
