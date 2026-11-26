use super::{http, modrinth_id, three_dm_project_page};
use crate::artifacts;

/// Reconstruct a public work page from a trusted, already recorded project ID.
/// IDs lacking the context required by a provider's work page have no fallback.
pub(crate) fn known_project_page(source_id: &str, project_id: &str) -> Option<String> {
    let numeric_id = |id: &str| {
        http::positive_id(id)
            .ok()
            .filter(|id| !id.starts_with('0'))
            .and_then(|id| id.parse::<u64>().ok())
    };
    let page = match source_id {
        "3dm" | "glossmod_3dm" => three_dm_project_page(project_id)?,
        "glossmod_http" => format!("https://www.glossmod.com/mods/{}", numeric_id(project_id)?),
        "modrinth" => format!("https://modrinth.com/mod/{}", modrinth_id(project_id).ok()?),
        "nexus" => {
            let (game, id) = project_id.split_once('/')?;
            let game = http::segment(game).ok()?.to_ascii_lowercase();
            format!("https://www.nexusmods.com/{game}/mods/{}", numeric_id(id)?)
        }
        "gamebanana" => format!("https://gamebanana.com/mods/{}", numeric_id(project_id)?),
        // This official route redirects without knowing the game, class or slug.
        "curseforge" => format!(
            "https://www.curseforge.com/projects/{}",
            numeric_id(project_id)?
        ),
        // Thunderstore stores namespace/name without its community.
        _ => return None,
    };
    artifacts::public_project_page(&page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_with_documented_id_routes_recover_a_public_work_page() {
        for (source, project, page) in [
            ("3dm", "1001", "https://mod.3dmgame.com/mod/1001"),
            ("glossmod_3dm", "1001", "https://mod.3dmgame.com/mod/1001"),
            ("glossmod_http", "243", "https://www.glossmod.com/mods/243"),
            ("modrinth", "A1b2C3d4", "https://modrinth.com/mod/A1b2C3d4"),
            (
                "nexus",
                "skyrimspecialedition/123",
                "https://www.nexusmods.com/skyrimspecialedition/mods/123",
            ),
            ("gamebanana", "101", "https://gamebanana.com/mods/101"),
            (
                "curseforge",
                "238222",
                "https://www.curseforge.com/projects/238222",
            ),
        ] {
            assert_eq!(known_project_page(source, project).as_deref(), Some(page));
        }
    }

    #[test]
    fn malformed_or_under_specified_project_ids_have_no_fallback_page() {
        for (source, project) in [
            ("3dm", "1001/2001"),
            ("3dm", "18446744073709551616"),
            ("glossmod_http", "0243"),
            ("glossmod_http", "243?key=hidden"),
            ("modrinth", "A1b2C3d4%2Fdownload"),
            ("modrinth", "https://modrinth.com/mod/A1b2C3d4"),
            ("nexus", "../123"),
            ("nexus", "skyrimspecialedition/123#123"),
            ("nexus", "123"),
            ("gamebanana", "0"),
            ("thunderstore", "Team/SampleMod"),
            ("curseforge", "0"),
            ("curseforge", "0238222"),
            ("curseforge", "238222/files/9004"),
            ("github", "owner/repo"),
            ("local", "1001"),
        ] {
            assert_eq!(
                known_project_page(source, project),
                None,
                "{source}/{project}"
            );
        }
    }
}
