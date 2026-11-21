//! User-data roots are host registrations, never paths supplied by adapters or
//! Agent tools. Registering one is an explicit desktop action.
use crate::{
    db::Db,
    error::{AppError, AppResult},
    games::{self, Game},
    plugins::{self, GameAdapter, Platform, RootKind},
    transaction::FileEngine,
    util,
};
use rusqlite::params;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub fn platform(game: &Game) -> AppResult<Platform> {
    match game.platform.as_str() {
        "windows" => Ok(Platform::Windows),
        "macos" => Ok(Platform::Macos),
        "linux" => Ok(Platform::Linux),
        _ => Err(AppError::Invalid("游戏平台尚未确认".into())),
    }
}

pub fn adapter(db: &Db, game: &Game) -> AppResult<GameAdapter> {
    plugins::adapter_for(
        db,
        game.adapter_slug
            .as_deref()
            .ok_or_else(|| AppError::Invalid("请先为游戏确认适配包".into()))?,
    )
}

pub fn registered_user_roots(db: &Db, game_id: &str) -> AppResult<BTreeMap<String, PathBuf>> {
    games::get_game(db, game_id)?;
    let conn = db.conn();
    let mut stmt = conn.prepare("SELECT root_id,path FROM game_user_roots WHERE game_id=?1")?;
    let rows = stmt.query_map([game_id], |r| {
        Ok((r.get(0)?, PathBuf::from(r.get::<_, String>(1)?)))
    })?;
    Ok(rows.collect::<Result<BTreeMap<_, _>, _>>()?)
}

pub fn register_user_root(
    engine: &FileEngine,
    game_id: &str,
    root_id: &str,
    path: &Path,
) -> AppResult<PathBuf> {
    let _guard = crate::transaction::write_lock();
    let game = games::get_game(&engine.db, game_id)?;
    let package = adapter(&engine.db, &game)?;
    let platform = platform(&game)?;
    let root = package
        .install_roots
        .iter()
        .find(|r| r.id == root_id && r.kind == RootKind::RegisteredUser)
        .ok_or_else(|| AppError::Invalid("不是适配包声明的用户管理根".into()))?;
    if !root.platforms.contains(&platform) {
        return Err(AppError::Unsupported("此管理根不支持游戏的平台".into()));
    }
    if !path.is_absolute() || !std::fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(AppError::Denied("请选择实际存在且非符号链接的目录".into()));
    }
    let path = path.canonicalize()?;
    let recorded_path = path
        .to_str()
        .ok_or_else(|| AppError::Invalid("用户管理根目录名无法无损表示为 UTF-8".into()))?;
    if path.parent().is_none() || engine.paths.overlaps_storage(&path) {
        return Err(AppError::Denied(
            "用户管理根不能包含管理器数据或整个文件系统".into(),
        ));
    }
    // Changing an existing mapping would invalidate historical restoration paths.
    let existing = registered_user_roots(&engine.db, game_id)?;
    if existing.get(root_id).is_some_and(|old| old != &path) {
        return Err(AppError::Conflict(
            "此管理根已登记到其他目录；保留历史恢复路径".into(),
        ));
    }
    engine.db.conn().execute(
        "INSERT INTO game_user_roots(game_id,root_id,path,created_at) VALUES (?1,?2,?3,?4) ON CONFLICT(game_id,root_id) DO NOTHING",
        params![game_id,root_id,recorded_path,util::now()],
    )?;
    Ok(path)
}
