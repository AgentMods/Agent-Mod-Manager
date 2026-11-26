//! GTA's loaders share extensions. Report independent payloads instead of picking
//! the first DLL, and never treat an archive edit as a loose-file installation.
use crate::error::{AppError, AppResult, NativeMessage};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Edition {
    Legacy,
    Enhanced,
}

impl Edition {
    pub(super) fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "gta5-legacy" => Some(Self::Legacy),
            "gta5-enhanced" => Some(Self::Enhanced),
            _ => None,
        }
    }

    pub(super) fn marker(self) -> &'static str {
        match self {
            Self::Legacy => "GTA5.exe",
            Self::Enhanced => "GTA5_Enhanced.exe",
        }
    }

    pub(super) fn supports_rpf_kind(self, kind: &str) -> bool {
        matches!(kind, "dlc" | "gameconfig")
            || self == Self::Legacy && matches!(kind, "vehicle" | "ped")
    }
}

pub(super) fn enhanced_rpf_unsupported() -> NativeMessage {
    NativeMessage::new("gta.rpf.enhanced_unsupported", [])
}

pub(super) fn is_gta(slug: &str) -> bool {
    Edition::from_slug(slug).is_some()
}

/// A saved binding remains in the library even if Steam has since removed or replaced the game.
pub(super) fn check_edition(game: &crate::games::Game, slug: &str) -> AppResult<()> {
    let Some(edition) = Edition::from_slug(slug) else {
        return Ok(());
    };
    let (other, app) = match edition {
        Edition::Legacy => (Edition::Enhanced, "271590"),
        Edition::Enhanced => (Edition::Legacy, "3240220"),
    };
    let expected = edition.marker();
    let root = Path::new(&game.root_path);
    let marker = root.join(expected);
    if game.platform != "windows"
        || game.steam_app_id.as_deref().is_some_and(|id| id != app)
        || root.join(other.marker()).try_exists()?
        || crate::safe_fs::open_regular(&marker)
            .and_then(|file| crate::safe_fs::verify_regular_at(&file, &marker))
            .is_err()
    {
        return Err(AppError::Conflict(format!(
            "无法确认 {slug} 的游戏目录；请检查 {expected} 及已登记版本。"
        )));
    }
    Ok(())
}

fn rpf_workflow_required() -> AppError {
    AppError::Presented(NativeMessage::new("gta.rpf.workflow_required", []))
}

pub(super) fn check_copy_type(slug: &str, ty: Option<&str>) -> AppResult<()> {
    if is_gta(slug) && ty.is_some_and(|t| t.starts_with("rpf-")) {
        return Err(rpf_workflow_required());
    }
    Ok(())
}

pub(super) fn check_copy_destination(slug: &str, root: &Path, dst: &Path) -> AppResult<()> {
    if !is_gta(slug) {
        return Ok(());
    }
    let Ok(rel) = dst.strip_prefix(root) else {
        return Ok(());
    };
    let rel = rel
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let base = rel.rsplit('/').next().unwrap_or_default();
    if rel.split('/').any(|s| s.ends_with(".rpf"))
        || rel == "menyoostuff/addedvehiclemodels.xml"
        || base == "gameconfig.xml"
        || [".yft", ".ydd", ".oiv"]
            .iter()
            .any(|ext| base.ends_with(ext))
        || base == "rpf.dll"
    {
        return Err(rpf_workflow_required());
    }
    Ok(())
}

pub(super) struct Hints {
    pub types: BTreeSet<&'static str>,
    pub review: bool,
    pub warnings: Vec<&'static str>,
}

pub(super) fn classify<'a>(names: impl IntoIterator<Item = &'a str>) -> Hints {
    let mut types = BTreeSet::new();
    let (mut dll, mut script_dir, mut ped, mut vehicle, mut hook, mut asi, mut tool, mut oiv) =
        (false, false, false, false, false, false, false, false);
    let (mut nconfig, mut ndlc) = (0, 0);
    let (mut ped_names, mut yft_names) = (BTreeSet::new(), BTreeSet::new());
    for raw in names {
        let path = raw.replace('\\', "/").to_ascii_lowercase();
        let base = path.rsplit('/').next().unwrap_or("");
        match base {
            "rpf.dll" => tool = true,
            "scripthookv.dll" => hook = true,
            "gameconfig.xml" => {
                types.insert("rpf-gameconfig");
                nconfig += 1;
            }
            "dlc.rpf" => {
                ndlc += 1;
                types.insert("rpf-dlc");
            }
            _ if base.ends_with(".asi") => asi = true,
            _ if base.ends_with(".oiv") => oiv = true,
            _ if base.ends_with(".ydd") => {
                ped = true;
                ped_names.insert(base.trim_end_matches(".ydd").to_string());
                ped_names.extend(path.split('/').rev().skip(1).take(1).map(str::to_string));
            }
            _ if base.ends_with(".yft") => {
                vehicle = true;
                yft_names.insert(
                    base.trim_end_matches(".yft")
                        .trim_end_matches("_hi")
                        .to_string(),
                );
            }
            _ if base.ends_with(".dll") || base.ends_with(".cs") || base.ends_with(".vb") => {
                dll = true;
                script_dir |= path.split('/').any(|c| c == "scripts");
            }
            _ => (),
        }
    }
    let mut warnings = Vec::new();
    if tool || oiv {
        warnings.push(if tool {
            "rpf.dll 不作为游戏 Mod 安装。GTA 归档请使用适配包的 RPF MCP。"
        } else {
            "OIV 包需用 OpenIV 人工审核和安装，AMM 不执行包内安装器。"
        });
        return Hints {
            types: BTreeSet::new(),
            review: true,
            warnings,
        };
    }
    if hook {
        types.insert("scripthookv");
    } else if asi {
        types.insert("asi");
    }
    if (dll && !hook && !asi) || script_dir {
        types.insert("script");
    }
    if ped {
        types.insert("rpf-ped");
    }
    if vehicle && (!ped || !yft_names.is_subset(&ped_names)) {
        types.insert("rpf-vehicle");
    }
    let review = types.len() != 1 || nconfig > 1 || ndlc > 1;
    if types.is_empty() {
        types.insert("game-root");
        warnings.push(
            "无法由文件布局确定安装位置。只在作者明确给出游戏根目录映射时手动选择，否则停止。",
        );
    } else if review {
        warnings.push("同时命中多种载荷或多个 gameconfig 变体。请先确认本版的组件和归档成员，不自动部署整包。");
    }
    Hints {
        types,
        review,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gta_manifests_are_valid_and_share_only_the_optional_program() {
        use crate::plugins::GameAdapter;
        let sources = [
            (
                "gta5-legacy",
                "271590",
                "GTA5.exe",
                include_str!("../../packages/gta-adapters/gta5-legacy/adapter.json"),
            ),
            (
                "gta5-enhanced",
                "3240220",
                "GTA5_Enhanced.exe",
                include_str!("../../packages/gta-adapters/gta5-enhanced/adapter.json"),
            ),
        ];
        let mut args = None;
        for (slug, app, marker, source) in sources {
            let a: GameAdapter = serde_json::from_str(source).unwrap();
            a.validate().unwrap();
            assert_eq!(a.slug, slug);
            assert_eq!(a.steam_app_id.as_deref(), Some(app));
            assert_eq!(a.markers.len(), 1);
            assert_eq!(a.markers[0].path, marker);
            assert!(a.markers[0].required);
            assert_eq!(a.mcp_servers.len(), 1);
            let server = &a.mcp_servers[0];
            assert_eq!(server.id, "rpf");
            let cfg = a.mcp_configs().unwrap().pop().unwrap();
            assert_eq!(cfg.id, format!("plugin_{slug}_rpf"));
            assert_eq!(cfg.plugin_slug.as_deref(), Some(slug));
            assert!(!cfg.enabled && !cfg.builtin);
            if let Some(previous) = args.replace(server.args.clone()) {
                assert_eq!(previous, server.args);
            }
        }
    }
    #[test]
    fn gta_types_distinguish_mixed_and_tool_archives() {
        for (names, expected, review) in [
            (
                vec!["bin/ScriptHookV.dll", "bin/NativeTrainer.asi"],
                vec!["scripthookv"],
                false,
            ),
            (
                vec!["kit\\EXAMPLE.ASI", "kit/helper.dll"],
                vec!["asi"],
                false,
            ),
            (
                vec!["sample/scripts/Foo.dll", "DLC/DLC.RPF"],
                vec!["rpf-dlc", "script"],
                true,
            ),
            (
                vec!["A/gameconfig.xml", "B/GameConfig.xml"],
                vec!["rpf-gameconfig"],
                true,
            ),
            (vec!["Ada.yft", "Ada/body_000.ydd"], vec!["rpf-ped"], false),
            (vec!["123/rpf.dll", "libs/RageLib.dll"], vec![], true),
            (vec!["package.oiv", "helper.dll"], vec![], true),
        ] {
            let hints = classify(names);
            assert_eq!(hints.types, expected.into_iter().collect());
            assert_eq!(hints.review, review);
        }
    }
    #[test]
    fn gta_does_not_merge_independent_assets_or_auto_pick_a_dlc_variant() {
        let mixed = classify(["Vehicles/taxi.yft", "Peds/ada.ydd"]);
        assert!(
            mixed.review && mixed.types.contains("rpf-ped") && mixed.types.contains("rpf-vehicle")
        );
        assert!(classify(["Legacy/DLC.rpf", "Enhanced/dlc.rpf"]).review);
    }
    #[test]
    fn gta_archives_cannot_be_installed_as_generic_game_files() {
        let game = Path::new("test/game");
        for dst in [
            "mods/update/update.RPF",
            "update/update.rpf/common/data/gameconfig.xml",
            "menyooStuff/AddedVehicleModels.xml",
            "scripts/gameconfig.xml",
            "mykit/NORMAL.YFT",
            "rpf.dll",
        ] {
            assert!(check_copy_destination("gta5-enhanced", game, &game.join(dst)).is_err());
            assert!(check_copy_destination("other-game", game, &game.join(dst)).is_ok());
        }
        assert!(check_copy_destination("gta5-legacy", game, &game.join("scripts/foo.dll")).is_ok());
    }
}
