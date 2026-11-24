//! Register an explicitly approved directory with the normal automatic game match.
use super::review::{digest, require_same, required_identity};
use super::RegistrationPreview;
use crate::{
    error::{AppError, AppResult},
    games::{self, Game, AUTOMATIC_GAME_NAME},
    paths,
    transaction::{self, FileEngine},
    util,
};
use rusqlite::{Connection, TransactionBehavior};
use std::path::{Path, PathBuf};

pub(crate) fn prepare_registration(
    engine: &FileEngine,
    directory: &Path,
) -> AppResult<RegistrationPreview> {
    let _guard = transaction::write_lock();
    let directory = directory_text(directory)?;
    let conn = engine.db.conn();
    Ok(review_registration(engine, &conn, directory, &util::new_id("game"))?.0)
}

pub(crate) fn apply_registration(
    engine: &FileEngine,
    approved: &RegistrationPreview,
) -> AppResult<Game> {
    let _guard = transaction::write_lock();
    let mut conn = engine.db.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (current, game) = review_registration(
        engine,
        &tx,
        &approved.requested_directory,
        &approved.game_id,
    )?;
    require_same(&current, approved)?;
    let game = games::apply_new_registration(&tx, &game)?;
    tx.commit()?;
    Ok(game)
}

/// The desktop continues to support refreshing a registered directory, and
/// may still have an explicit or discovery-suggested adapter selection.
pub(crate) fn register_local_from_desktop(
    engine: &FileEngine,
    directory: &Path,
    adapter_slug: Option<&str>,
    from_discovery: bool,
) -> AppResult<Game> {
    let _guard = transaction::write_lock();
    let root = desktop_registration_root(engine, directory)?;
    if from_discovery {
        games::register_discovered_game(&engine.db, &root, adapter_slug)
    } else {
        games::register_game(&engine.db, &root, adapter_slug)
    }
}

fn review_registration(
    engine: &FileEngine,
    conn: &Connection,
    requested_directory: &str,
    game_id: &str,
) -> AppResult<(RegistrationPreview, Game)> {
    let directory = Path::new(requested_directory);
    let root = checked_registration_root(engine, directory)?;
    let identity = required_identity(&root)?;
    let (game, adapter) = games::prepare_new_registration(conn, &root, game_id)?;
    require_same(&checked_registration_root(engine, directory)?, &root)?;
    require_same(&required_identity(&root)?, &identity)?;
    // Timestamps belong to the eventual commit, not the reviewed action. Bind
    // the exact disk match and validated adapter, even when its version is unchanged.
    let state_sha256 = digest(&(
        &game.id,
        &game.name,
        requested_directory,
        &game.root_path,
        identity,
        &game.slug,
        &game.platform,
        game.confidence.to_bits(),
        &game.steam_app_id,
        game.metadata.get(AUTOMATIC_GAME_NAME),
        game.metadata.get("lastIdentification"),
        adapter,
    ))?;
    Ok((
        RegistrationPreview {
            game_id: game.id.clone(),
            game_name: game.name.clone(),
            root_path: paths::display_path(&root),
            platform: game.platform.clone(),
            adapter_slug: game.adapter_slug.clone(),
            adapter_version: game.adapter_version.clone(),
            steam_app_id: game.steam_app_id.clone(),
            requested_directory: requested_directory.to_owned(),
            state_sha256,
        },
        game,
    ))
}

fn desktop_registration_root(engine: &FileEngine, directory: &Path) -> AppResult<PathBuf> {
    let root = directory.canonicalize()?;
    if engine.paths.overlaps_storage(&root) {
        return Err(AppError::Denied(
            "游戏目录与管理器数据目录不能互相包含".into(),
        ));
    }
    Ok(root)
}

fn checked_registration_root(engine: &FileEngine, directory: &Path) -> AppResult<PathBuf> {
    if !directory.is_absolute() {
        return Err(AppError::Invalid("请选择绝对目录路径".into()));
    }
    let root = desktop_registration_root(engine, directory)?;
    if root.parent().is_none() {
        return Err(AppError::Invalid(
            "不能将整个文件系统根登记为游戏目录".into(),
        ));
    }
    // Registering also grants the file engine scoped access to this directory.
    super::checked_game_root(&engine.paths, &root)
}

fn directory_text(path: &Path) -> AppResult<&str> {
    path.to_str()
        .ok_or_else(|| AppError::Invalid("目录名无法无损表示为 UTF-8".into()))
}
