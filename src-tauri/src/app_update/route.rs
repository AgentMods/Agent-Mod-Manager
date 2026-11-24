use crate::{
    db::Db,
    error::{AppError, AppResult, NativeMessage},
};
use reqwest_mcp::{redirect::Policy, ClientBuilder};
use rusqlite::OptionalExtension;
use serde_json::Value;
use url::Url;

const SETTING_KEY: &str = "app_update_route";
const DEFAULT_MIRROR: &str = "cdn.gh-proxy.org";
const MIRRORS: [&str; 5] = [
    DEFAULT_MIRROR,
    "gh-proxy.org",
    "ghfast.top",
    "gh-proxy.com",
    "ghproxy.net",
];
const GITHUB: &str = "github.com";
const GITHUB_ASSETS: &str = "release-assets.githubusercontent.com";
const LATEST_MANIFEST: &str = "/releases/latest/download/latest.json";
const RELEASE_DOWNLOAD: &str = "/releases/download/";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct UpdateRoute(Option<&'static str>);

impl Default for UpdateRoute {
    fn default() -> Self {
        Self(Some(DEFAULT_MIRROR))
    }
}

impl UpdateRoute {
    pub(super) const DIRECT: Self = Self(None);

    fn parse(value: &str) -> Option<Self> {
        if value == "direct" {
            Some(Self::DIRECT)
        } else {
            MIRRORS
                .into_iter()
                .find(|host| *host == value)
                .map(|host| Self(Some(host)))
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        self.0.unwrap_or("direct")
    }

    pub(super) fn load(db: &Db) -> AppResult<Self> {
        let saved: Option<String> = db
            .conn()
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                [SETTING_KEY],
                |row| row.get(0),
            )
            .optional()?;
        Ok(saved
            .and_then(|raw| serde_json::from_str::<String>(&raw).ok())
            .and_then(|route| Self::parse(&route))
            .unwrap_or_default())
    }

    pub(super) fn save(db: &Db, value: &str) -> AppResult<Self> {
        let route = Self::parse(value).ok_or_else(|| {
            AppError::Presented(NativeMessage::new("app_update.route_unsupported", []))
        })?;
        db.conn().execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            (SETTING_KEY, serde_json::to_string(route.as_str())?),
        )?;
        Ok(route)
    }

    pub(super) fn apply(self, origin: &Url) -> Url {
        match self.0 {
            Some(host) => Url::parse(&format!("https://{host}/{origin}"))
                .expect("validated GitHub URL and static mirror hostname"),
            None => origin.clone(),
        }
    }

    fn unroute(self, url: &Url) -> Option<Url> {
        match self.0 {
            Some(host) if exact_origin(url, host) && url.query().is_none() => {
                Url::parse(url.path().strip_prefix('/')?).ok()
            }
            Some(_) => None,
            None => Some(url.clone()),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ReleaseSource {
    pub(super) manifest: Url,
    repo_path: String,
}

impl ReleaseSource {
    pub(super) fn from_config(config: Option<&Value>) -> Option<Self> {
        let config = config?;
        if config["pubkey"].as_str()?.trim().is_empty()
            || config["requireSignedVersion"].as_bool() != Some(true)
        {
            return None;
        }
        let [endpoint] = config["endpoints"].as_array()?.as_slice() else {
            return None;
        };
        let endpoint = endpoint.as_str()?;
        let manifest = Url::parse(endpoint).ok()?;
        if manifest.as_str() != endpoint || !github_release_url(&manifest) {
            return None;
        }
        let repo_path = manifest.path().strip_suffix(LATEST_MANIFEST)?.to_owned();
        let (owner, repo) = repo_path.strip_prefix('/')?.split_once('/')?;
        if !safe_segment(owner) || !safe_segment(repo) {
            return None;
        }
        Some(Self {
            manifest,
            repo_path,
        })
    }

    fn download_parts<'a>(&self, url: &'a Url) -> Option<(&'a str, &'a str)> {
        if !github_release_url(url) {
            return None;
        }
        let (tag, asset) = url
            .path()
            .strip_prefix(&self.repo_path)?
            .strip_prefix(RELEASE_DOWNLOAD)?
            .split_once('/')?;
        (safe_segment(tag) && safe_segment(asset)).then_some((tag, asset))
    }

    pub(super) fn artifact(
        &self,
        route: UpdateRoute,
        announced: &Url,
        version: &str,
    ) -> Option<Url> {
        let (tag, _) = self.download_parts(announced)?;
        (tag.strip_prefix('v') == Some(version)).then(|| route.apply(announced))
    }

    pub(super) fn configure_client(
        &self,
        route: UpdateRoute,
        client: ClientBuilder,
    ) -> ClientBuilder {
        let source = self.clone();
        client
            .https_only(true)
            .tls_danger_accept_invalid_certs(false)
            .tls_danger_accept_invalid_hostnames(false)
            .redirect(Policy::custom(move |attempt| {
                if attempt.previous().len() >= 10 {
                    attempt.error("too many application update redirects")
                } else if source.allows_redirect(route, attempt.previous(), attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.error("invalid application update redirect")
                }
            }))
    }

    pub(super) fn allows_redirect(&self, route: UpdateRoute, previous: &[Url], next: &Url) -> bool {
        let (Some(first), Some(last)) = (previous.first(), previous.last()) else {
            return false;
        };
        let Some(first) = route.unroute(first) else {
            return false;
        };
        if first != self.manifest && self.download_parts(&first).is_none() {
            return false;
        }
        // Direct GitHub release links redirect to a signed, expiring asset URL.
        // Never let a third-party mirror redirect into another origin.
        if route == UpdateRoute::DIRECT
            && (last == &self.manifest || self.download_parts(last).is_some())
            && exact_origin(next, GITHUB_ASSETS)
            && next.path().starts_with("/github-production-release-asset/")
        {
            return true;
        }

        let Some(next) = route.unroute(next) else {
            return false;
        };
        if first == self.manifest {
            // A mirror may preserve GitHub's redirect from latest to a tagged manifest.
            self.download_parts(&next)
                .is_some_and(|(tag, asset)| tag.starts_with('v') && asset == "latest.json")
        } else {
            // An installer URL already identifies a tagged asset. Do not follow a
            // redirect that changes its source repository, version, or filename.
            next == first
        }
    }
}

fn exact_origin(url: &Url, host: &str) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some(host)
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

fn github_release_url(url: &Url) -> bool {
    exact_origin(url, GITHUB) && url.query().is_none()
}

fn safe_segment(segment: &str) -> bool {
    !matches!(segment, "" | "." | "..")
        && segment
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"-_.".contains(&ch))
}
