use super::{invalid, validate_slug, GameAdapter};
use crate::{
    error::AppResult,
    mcp::{McpTransport, ServerConfig},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// The package declares credential inputs. The manager assigns their storage keys
/// and owns the server identity, activation and launch consent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginMcpServer {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub transport: McpTransport,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub url: Option<String>,
    pub credential_url: Option<String>,
    #[serde(default)]
    pub bearer_token: bool,
    #[serde(default)]
    pub env_secrets: Vec<String>,
}

impl GameAdapter {
    pub(crate) fn mcp_configs(&self) -> AppResult<Vec<ServerConfig>> {
        let mut ids = BTreeSet::new();
        self.mcp_servers
            .iter()
            .map(|server| {
                validate_slug(&server.id)?;
                if !ids.insert(&server.id) {
                    return Err(invalid(format!("适配包内的工具标识重复：{}", server.id)));
                }
                server.config(&self.slug)
            })
            .collect()
    }
}

impl PluginMcpServer {
    fn config(&self, slug: &str) -> AppResult<ServerConfig> {
        // Neither slug permits underscores, so different pairs stay distinct.
        let mut id = format!("plugin_{slug}_{}", self.id);
        if id.len() > 48 {
            let digest = Sha256::digest(id.as_bytes());
            id = format!("plugin_{}", hex::encode(&digest[..20]));
        }
        let mut env_secret_refs = BTreeMap::new();
        for env in &self.env_secrets {
            // A valid environment name can be 96 bytes, but store keys are limited to 128.
            let mut key = format!("mcp.{id}.{}", env.to_ascii_lowercase());
            if key.len() > 128 {
                key = format!("mcp.{id}.{}", hex::encode(Sha256::digest(env.as_bytes())));
            }
            if env_secret_refs.insert(env.clone(), key).is_some() {
                return Err(invalid(format!("工具 {} 的凭据环境变量重复", self.id)));
            }
        }
        let config = ServerConfig {
            name: self.name.clone(),
            description: self.description.clone(),
            transport: self.transport,
            command: self.command.clone(),
            args: self.args.clone(),
            url: self.url.clone(),
            credential_url: self.credential_url.clone(),
            bearer_secret_ref: self.bearer_token.then(|| format!("mcp.{id}.bearer")),
            env_secret_refs,
            plugin_slug: Some(slug.into()),
            id,
            builtin: false,
            enabled: false,
        };
        config
            .validate()
            .map_err(|err| invalid(format!("工具 {} 配置无效：{err}", self.id)))?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn declarations_reject_authority_fields_and_mixed_transports() {
        let base = json!({"id":"catalog", "name":"Catalog", "transport":"streamable_http", "url":"https://example.test/mcp"});
        for (key, value) in [
            ("enabled", json!(true)),
            ("pluginSlug", json!("other")),
            ("bearerSecretRef", json!("mcp.other.key")),
            ("env", json!({"SOME_TOKEN":"value"})),
        ] {
            let mut declaration = base.clone();
            declaration[key] = value;
            assert!(serde_json::from_value::<PluginMcpServer>(declaration).is_err());
        }
        let mut mixed: PluginMcpServer = serde_json::from_value(base).unwrap();
        mixed.command = Some("tool".into());
        assert!(mixed.config("example").is_err());
    }

    #[test]
    fn declarations_accept_optional_display_description() {
        let mut json = json!({
            "id":"catalog", "name":"Catalog", "transport":"streamable_http", "url":"https://example.test/mcp"
        });
        let legacy: PluginMcpServer = serde_json::from_value(json.clone()).unwrap();
        assert!(legacy.config("example").unwrap().description.is_none());
        json["description"] = json!("浏览模组目录");
        let declaration: PluginMcpServer = serde_json::from_value(json.clone()).unwrap();
        let config = declaration.config("example").unwrap();
        assert_eq!(config.description.as_deref(), Some("浏览模组目录"));
        assert_eq!(
            serde_json::to_value(declaration).unwrap()["description"],
            json["description"]
        );
        assert!(!config.enabled && !config.builtin);
    }

    #[test]
    fn long_plugin_names_get_stable_distinct_ids_and_isolated_credentials() {
        let declaration: PluginMcpServer = serde_json::from_value(json!({
            "id":"catalog", "name":"Catalog", "transport":"stdio", "command":"catalog-mcp", "envSecrets":["CATALOG_API_KEY"]
        })).unwrap();
        let mut declaration = declaration;
        declaration
            .env_secrets
            .push(format!("{}_TOKEN", "A".repeat(90)));
        let first = "a".repeat(64);
        let config = declaration.config(&first).unwrap();
        let other = declaration.config(&"b".repeat(64)).unwrap();
        assert_eq!(declaration.config(&first).unwrap(), config);
        assert_ne!(config.id, other.id);
        assert_ne!(config.env_secret_refs, other.env_secret_refs);
        assert!(!config.enabled && !config.builtin);
    }
}
