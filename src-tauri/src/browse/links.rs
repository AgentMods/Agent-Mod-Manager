use url::Url;

pub(super) fn public_text(value: &str, bytes: usize) -> Option<String> {
    let mut text = String::new();
    for character in value.chars().take(bytes.saturating_mul(4)) {
        let character = match character {
            '\n' | '\r' | '\t' => ' ',
            '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' => {
                continue;
            }
            character if character.is_control() => continue,
            character => character,
        };
        if text.len() + character.len_utf8() > bytes {
            break;
        }
        text.push(character);
    }
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

pub(super) fn valid_provider_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 160
        && id.split(['/', '~', ':']).all(|part| {
            !part.is_empty()
                && part.as_bytes().iter().any(u8::is_ascii_alphanumeric)
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        })
}

fn official_https(raw: &str) -> Option<Url> {
    if !raw.starts_with("https://")
        || raw.len() > 1024
        || raw.chars().any(|c| c.is_control() || c == '\\')
        || raw[8..].split(['/', '?', '#']).next()?.contains('@')
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    (url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none())
    .then_some(url)
}

pub(super) fn project(
    source: &str,
    provider_game: &str,
    project_id: &str,
    raw: &str,
) -> Option<String> {
    let url = official_https(raw)?;
    // Provider adapters first establish that the URL belongs to the returned work.
    // This is a second egress check; no arbitrary host, query or non-work path.
    if url.query().is_some() || url.fragment().is_some() || raw.contains('%') {
        return None;
    }
    let parts = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
    if parts.iter().any(|part| !valid_provider_id(part)) {
        return None;
    }
    let numeric = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let valid = match (source, url.host_str()?, parts.as_slice()) {
        ("glossmod_http", "www.glossmod.com" | "glossmod.com", ["mods", id]) => {
            numeric(id) && project_id == *id
        }
        ("glossmod_3dm", "mod.3dmgame.com", ["mod", id]) => numeric(id) && project_id == *id,
        ("modio", "mod.io" | "www.mod.io", ["g", _, "m", _]) => true,
        (
            "curseforge",
            "www.curseforge.com" | "curseforge.com",
            [_, "mc-mods" | "mods" | "addons" | "texture-packs" | "resource-packs" | "modpacks"
            | "worlds" | "data-packs" | "shaders" | "customization" | "plugins" | "bukkit-plugins", _],
        ) => true,
        ("gamebanana", "gamebanana.com" | "www.gamebanana.com", ["mods", id]) => {
            numeric(id) && project_id == *id
        }
        ("nexusmods", "nexusmods.com" | "www.nexusmods.com", [game, "mods", id]) => {
            *game == provider_game
                && numeric(id)
                && project_id
                    .strip_prefix(*game)
                    .and_then(|suffix| suffix.strip_prefix('/'))
                    == Some(*id)
        }
        (
            "thunderstore",
            "thunderstore.io" | "www.thunderstore.io",
            ["c", community, "p", namespace, name],
        ) => {
            *community == provider_game
                && (project_id == format!("{namespace}-{name}")
                    || project_id == format!("{namespace}/{name}"))
        }
        _ => false,
    };
    valid.then(|| url.into())
}

pub(super) fn cover(source: &str, raw: &str) -> Option<String> {
    let mut url = official_https(raw)?;
    // Never return source-supplied query parameters / tokens to the webview.
    url.set_query(None);
    url.set_fragment(None);
    if source == "glossmod_3dm" {
        return super::providers::cover(crate::mcp::browse::Source::ThreeDm, Some(url.as_str()));
    }
    let host = url.host_str()?;
    let valid = match source {
        "glossmod_http" => matches!(host, "r2.glossmod.com" | "cdn.glossmod.com"),
        "modio" => host.ends_with(".modcdn.io"),
        "curseforge" => matches!(
            host,
            "mediafilez.forgecdn.net" | "media.forgecdn.net" | "edge.forgecdn.net"
        ),
        "gamebanana" => matches!(host, "images.gamebanana.com" | "files.gamebanana.com"),
        "nexusmods" => matches!(
            host,
            "staticdelivery.nexusmods.com"
                | "images.nexusmods.com"
                | "staticdelivery.nexus-cdn.com"
        ),
        "thunderstore" => matches!(
            host,
            "ccdn.thunderstore.io" | "gcdn.thunderstore.io" | "cdn.thunderstore.io"
        ),
        _ => false,
    };
    let public_image = url.path().rsplit_once('.').is_some_and(|(_, extension)| {
        ["jpg", "jpeg", "png", "webp", "avif", "gif"]
            .iter()
            .any(|allowed| extension.eq_ignore_ascii_case(allowed))
    });
    (valid && public_image).then(|| url.into())
}
