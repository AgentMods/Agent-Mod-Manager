//! Explicit launch selection is desktop-only. Agent/MCP approval continues to
//! use the legacy direct/Steam decision, without consulting these presets or metadata.
use super::{
    checked_game_root, configured_launch_program, decide_launch, launch_program, review,
    steam_launch_url, validate_launch_path, LaunchAction,
};
use crate::{
    error::{AppError, AppResult, NativeMessage},
    games::{self, Game},
    paths,
    storage::AppPaths,
    transaction::{self, FileEngine},
    util,
};
use rusqlite::{params, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

const MOD_LAUNCHER_KEY: &str = "modLauncher";

#[derive(Debug, Serialize)]
pub struct GameLaunchOption {
    id: &'static str,
    label: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModLauncher {
    name: String,
    path: String,
    args: Vec<String>,
}

impl ModLauncher {
    fn from_json(value: Value) -> AppResult<Option<Self>> {
        serde_json::from_value(value)
            .map_err(|_| AppError::Presented(NativeMessage::new("game.mod_launcher_invalid", [])))
    }

    fn validate(&mut self, paths: &AppPaths, root: &Path) -> AppResult<PathBuf> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err(AppError::Presented(NativeMessage::new(
                "game.mod_launcher_name_invalid",
                [],
            )));
        }
        self.name = name.to_owned();
        if self.args.iter().any(|arg| arg.contains('\0')) {
            return Err(AppError::Presented(NativeMessage::new(
                "game.mod_launcher_args_invalid",
                [],
            )));
        }
        let selected = Path::new(&self.path);
        #[cfg(windows)]
        if !super::has_extension(selected, "exe") {
            return Err(AppError::Invalid("Windows 启动程序必须是 .exe 文件".into()));
        }
        let launch = validate_launch_path(paths, root, selected)?;
        self.path = launch
            .selected
            .into_os_string()
            .into_string()
            .map_err(|_| AppError::Invalid("启动程序路径包含无法表示的字符".into()))?;
        Ok(launch.program)
    }
}

pub(crate) fn set_game_mod_launcher(
    engine: &FileEngine,
    id: &str,
    launcher: Value,
) -> AppResult<Game> {
    let launcher = ModLauncher::from_json(launcher)?;
    let _guard = transaction::write_lock();
    let mut conn = engine.db.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut game = games::get_game_from(&tx, id)?;
    let root = Path::new(&game.root_path);
    let entry = if let Some(mut launcher) = launcher {
        launcher.validate(&engine.paths, root)?;
        Some(serde_json::to_value(launcher)?)
    } else {
        // Clearing a preference opens no filesystem target and must remain
        // possible after the game or its launcher has been uninstalled.
        None
    };
    let metadata = game
        .metadata
        .as_object_mut()
        .ok_or_else(|| AppError::Invalid("现有游戏 metadata 必须是 JSON 对象".into()))?;
    if let Some(entry) = entry {
        metadata.insert(MOD_LAUNCHER_KEY.into(), entry);
    } else {
        metadata.remove(MOD_LAUNCHER_KEY);
    }
    game.updated_at = util::now();
    tx.execute(
        "UPDATE games SET metadata_json=?1,updated_at=?2 WHERE id=?3",
        params![game.metadata.to_string(), game.updated_at, game.id],
    )?;
    tx.commit()?;
    Ok(game)
}

#[derive(Debug, Clone, Copy)]
enum Method {
    Steam,
    Direct,
    Smapi,
    ModEngine2,
    Custom,
}

impl Method {
    const ORDER: [Self; 5] = [
        Self::Steam,
        Self::Direct,
        Self::Smapi,
        Self::ModEngine2,
        Self::Custom,
    ];

    fn id(self) -> &'static str {
        match self {
            Self::Steam => "steam",
            Self::Direct => "direct",
            Self::Smapi => "smapi",
            Self::ModEngine2 => "mod-engine-2",
            Self::Custom => "custom",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum DesktopAction {
    Standard(LaunchAction),
    Mod { program: PathBuf, args: Vec<String> },
}

pub(crate) fn list_game_launch_options(
    engine: &FileEngine,
    id: &str,
) -> AppResult<Vec<GameLaunchOption>> {
    let _guard = transaction::write_lock();
    let game = games::get_game(&engine.db, id)?;
    let root = checked_game_root(&engine.paths, Path::new(&game.root_path))?;
    Ok(Method::ORDER
        .into_iter()
        .filter_map(|method| {
            resolve_method(&engine.paths, &game, &root, method)
                .ok()
                .flatten()
                .map(|(_, label)| GameLaunchOption {
                    id: method.id(),
                    label,
                })
        })
        .collect())
}

pub(crate) fn launch_from_desktop(
    engine: &FileEngine,
    id: &str,
    method: Option<&str>,
) -> AppResult<()> {
    launch_with(engine, id, method, |action| match action {
        DesktopAction::Standard(action) => review::dispatch_launch(action),
        DesktopAction::Mod { program, args } => launch_program(&program, &args),
    })
}

fn launch_with(
    engine: &FileEngine,
    id: &str,
    method: Option<&str>,
    dispatch: impl FnOnce(DesktopAction) -> AppResult<()>,
) -> AppResult<()> {
    let _guard = transaction::write_lock();
    let game = games::get_game(&engine.db, id)?;
    let Some(method) = method else {
        return dispatch(DesktopAction::Standard(decide_launch(
            &engine.paths,
            &game,
        )?));
    };
    let selected = Method::ORDER
        .into_iter()
        .find(|candidate| candidate.id() == method)
        .ok_or_else(|| method_error("game.launch_method_unknown", method))?;
    let root = checked_game_root(&engine.paths, Path::new(&game.root_path))?;
    let (action, _) = resolve_method(&engine.paths, &game, &root, selected)?
        .ok_or_else(|| method_error("game.launch_method_unavailable", method))?;
    dispatch(action)
}

fn method_error(key: &str, id: &str) -> AppError {
    AppError::Presented(NativeMessage::new(key, [("method", id.to_owned())]))
}

fn resolve_method(
    paths: &AppPaths,
    game: &Game,
    root: &Path,
    method: Method,
) -> AppResult<Option<(DesktopAction, Option<String>)>> {
    let action = match method {
        Method::Steam => steam_launch_url(game)?.map(LaunchAction::Steam),
        Method::Direct => match configured_launch_program(paths, game)? {
            Some(program) => Some(LaunchAction::Program(program)),
            None => vanilla_program(paths, game, root).map(LaunchAction::Program),
        },
        Method::Smapi | Method::ModEngine2 => {
            return Ok(mod_preset(paths, game, root, method).map(|action| (action, None)));
        }
        Method::Custom => {
            let Some(mut launcher) = ModLauncher::from_json(
                game.metadata
                    .get(MOD_LAUNCHER_KEY)
                    .cloned()
                    .unwrap_or(Value::Null),
            )?
            else {
                return Ok(None);
            };
            let program = launcher.validate(paths, root)?;
            return Ok(Some((
                DesktopAction::Mod {
                    program,
                    args: launcher.args,
                },
                Some(launcher.name),
            )));
        }
    };
    Ok(action.map(|action| (DesktopAction::Standard(action), None)))
}

fn vanilla_program(paths: &AppPaths, game: &Game, root: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    return windows_presets::stardew_program(paths, game, root)
        .or_else(|| windows_presets::elden_ring_program(paths, game, root, false));
    #[cfg(not(windows))]
    {
        let _ = (paths, game, root);
        None
    }
}

fn mod_preset(paths: &AppPaths, game: &Game, root: &Path, method: Method) -> Option<DesktopAction> {
    #[cfg(windows)]
    match method {
        Method::Smapi => return windows_presets::smapi(paths, game, root),
        Method::ModEngine2 => return windows_presets::mod_engine_2(paths, game, root),
        _ => {}
    }
    let _ = (paths, game, root, method);
    None
}

// Do not mistake the Unix SMAPI entrypoint named StardewValley for a vanilla
// game executable, or assume a Windows installer can run under Wine/Proton.
#[cfg(windows)]
mod windows_presets {
    use super::super::guarded_launch_path;
    use super::*;

    const STARDEW_STEAM_ID: &str = "413150";
    const ELDEN_RING_STEAM_ID: &str = "1245620";

    /// A preset is an exact layout, not permission to follow redirects. This
    /// also means relative configs are read beside the executable we launch.
    fn layout_file(paths: &AppPaths, root: &Path, selected: &Path) -> Option<PathBuf> {
        let guarded = guarded_launch_path(paths, root, selected).ok()?;
        (paths::contains_path(selected, &guarded)
            && paths::contains_path(&guarded, selected)
            && guarded.is_file())
        .then_some(guarded)
    }

    fn program(paths: &AppPaths, root: &Path, selected: &Path) -> Option<PathBuf> {
        let file = layout_file(paths, root, selected)?;
        Some(validate_launch_path(paths, root, &file).ok()?.program)
    }

    pub(super) fn stardew_program(paths: &AppPaths, game: &Game, root: &Path) -> Option<PathBuf> {
        let exe = program(paths, root, &root.join("Stardew Valley.exe"))?;
        (game.steam_app_id.as_deref() == Some(STARDEW_STEAM_ID)
            || layout_file(paths, root, &root.join("Stardew Valley.dll")).is_some())
        .then_some(exe)
    }

    pub(super) fn elden_ring_program(
        paths: &AppPaths,
        game: &Game,
        root: &Path,
        standard_layout_only: bool,
    ) -> Option<PathBuf> {
        let standard = root.join("Game").join("eldenring.exe");
        let flat = root.join("eldenring.exe");
        // A registration may stop at the Steam installation or select its
        // actual Game directory. Both reconstruct the same upstream target.
        let is_game_directory = root.file_name().is_some_and(|name| name == "Game");
        std::iter::once(&standard)
            .chain((!standard_layout_only || is_game_directory).then_some(&flat))
            .find_map(|selected| {
                let exe = program(paths, root, selected)?;
                (game.steam_app_id.as_deref() == Some(ELDEN_RING_STEAM_ID)
                    || layout_file(paths, root, &exe.parent()?.join("Data0.bdt")).is_some())
                .then_some(exe)
            })
    }

    pub(super) fn smapi(paths: &AppPaths, game: &Game, root: &Path) -> Option<DesktopAction> {
        stardew_program(paths, game, root)?;
        for file in [
            "Stardew Valley.dll",
            "StardewModdingAPI.dll",
            "StardewModdingAPI.deps.json",
            "StardewModdingAPI.runtimeconfig.json",
        ] {
            layout_file(paths, root, &root.join(file))?;
        }
        layout_file(
            paths,
            root,
            &root.join("smapi-internal").join("config.json"),
        )?;
        Some(DesktopAction::Mod {
            program: program(paths, root, &root.join("StardewModdingAPI.exe"))?,
            args: Vec::new(),
        })
    }

    pub(super) fn mod_engine_2(
        paths: &AppPaths,
        game: &Game,
        root: &Path,
    ) -> Option<DesktopAction> {
        // Upstream 2.1.0 takes the executable's grandparent and then appends
        // Game/eldenring.exe. An explicit path avoids looking up some other
        // copy via Steam; it cannot pin a nonstandard layout safely.
        let vanilla = elden_ring_program(paths, game, root, true)?;
        for base in [root, vanilla.parent()?] {
            for directory in [
                base.to_path_buf(),
                base.join("ModEngine2"),
                base.join("ModEngine-2.1.0.0-win64"),
            ] {
                let Some(program) =
                    program(paths, root, &directory.join("modengine2_launcher.exe"))
                else {
                    continue;
                };
                let parent = program.parent()?;
                if layout_file(paths, root, &parent.join("config_eldenring.toml")).is_none()
                    || layout_file(
                        paths,
                        root,
                        &parent.join("modengine2").join("bin").join("modengine2.dll"),
                    )
                    .is_none()
                {
                    continue;
                }
                return Some(DesktopAction::Mod {
                    program,
                    args: ["-t", "er", "-c", "config_eldenring.toml", "--game-path"]
                        .map(str::to_owned)
                        .into_iter()
                        .chain([paths::user_path(&vanilla).to_str()?.to_owned()])
                        .collect(),
                });
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "desktop_launch_tests.rs"]
mod tests;
