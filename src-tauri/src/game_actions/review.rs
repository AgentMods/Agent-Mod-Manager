//! Approval previews and commits. Callers keep the prepared value across the
//! approval wait; it is not a new input to be supplied by the model or IPC.
use super::{
    checked_game_root, decide_launch, has_pending_review, launch_program, validate_launch_path,
    validate_name, GameChange, LaunchAction, LaunchPathEdit, LaunchPathUpdate, LaunchPreview,
    LaunchTarget, RemovalPreview, RemovalUserRoot, UpdatePreview,
};
use crate::{
    error::{AppError, AppResult},
    games::{self, Game, AUTOMATIC_GAME_NAME},
    install::INSTALL_STATE,
    paths,
    safe_fs::{self, FileIdentity},
    transaction::{self, FileEngine},
    util,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn digest(state: &impl Serialize) -> AppResult<String> {
    util::sha256_reader(&mut serde_json::to_vec(state)?.as_slice())
}

pub(super) fn require_same<T: PartialEq>(current: &T, approved: &T) -> AppResult<()> {
    if current != approved {
        return Err(AppError::Conflict(
            "游戏或操作目标在审批期间已变化，请重新确认".into(),
        ));
    }
    Ok(())
}

pub(super) fn required_identity(path: &Path) -> AppResult<FileIdentity> {
    safe_fs::identity(path)?.ok_or_else(|| {
        AppError::Conflict(format!("无法核对路径对象: {}", paths::display_path(path)))
    })
}

fn program_evidence(program: &Path) -> AppResult<(FileIdentity, String)> {
    let before = required_identity(program)?;
    let hash = util::sha256_file(program)?;
    require_same(&required_identity(program)?, &before)?;
    Ok((before, hash))
}

fn launch_path(game: &Game) -> Option<&Value> {
    game.metadata.get("launchPath")
}

fn display_launch_path(game: &Game) -> Option<String> {
    launch_path(game)
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(|path| paths::display_path(Path::new(path)))
}

pub(crate) fn prepare_update(
    engine: &FileEngine,
    id: &str,
    name: Option<&str>,
    launch_path: LaunchPathUpdate<'_>,
) -> AppResult<UpdatePreview> {
    let _guard = transaction::write_lock();
    let game = games::get_game(&engine.db, id)?;
    Ok(review_update(engine, game, name, launch_path, true)?.0)
}

pub(crate) fn apply_update(engine: &FileEngine, approved: &UpdatePreview) -> AppResult<Game> {
    commit_update(
        engine,
        &approved.game_id,
        (approved.target_game_name != approved.game_name)
            .then_some(approved.target_game_name.as_str()),
        approved.launch_path_edit.as_update(),
        Some(approved),
    )
}

pub(super) fn commit_update(
    engine: &FileEngine,
    id: &str,
    name: Option<&str>,
    launch_path: LaunchPathUpdate<'_>,
    approved: Option<&UpdatePreview>,
) -> AppResult<Game> {
    let _guard = transaction::write_lock();
    let mut conn = engine.db.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let game = games::get_game_from(&tx, id)?;
    let (current, mut game) = review_update(engine, game, name, launch_path, approved.is_some())?;
    if let Some(approved) = approved {
        require_same(&current, approved)?;
    }
    game.updated_at = util::now();
    tx.execute(
        "UPDATE games SET name=?1,metadata_json=?2,updated_at=?3 WHERE id=?4",
        params![
            game.name,
            game.metadata.to_string(),
            game.updated_at,
            game.id
        ],
    )?;
    tx.commit()?;
    Ok(game)
}

fn review_update(
    engine: &FileEngine,
    mut game: Game,
    name: Option<&str>,
    requested_launch_path: LaunchPathUpdate<'_>,
    pin_executable: bool,
) -> AppResult<(UpdatePreview, Game)> {
    let target_game_name = name
        .map(validate_name)
        .transpose()?
        .unwrap_or(&game.name)
        .to_owned();
    let renaming = target_game_name != game.name;
    let root = Path::new(&game.root_path);
    let root_path = paths::display_path(root);
    let current_launch_path = display_launch_path(&game);
    let mut target_launch_path = current_launch_path.clone();
    let (launch_path_edit, selected, target_evidence) = match requested_launch_path {
        LaunchPathUpdate::Keep => (LaunchPathEdit::Keep, None, None),
        LaunchPathUpdate::Clear => {
            target_launch_path = None;
            (LaunchPathEdit::Clear, None, None)
        }
        LaunchPathUpdate::Set(path) => {
            let launch = validate_launch_path(&engine.paths, root, Path::new(path))?;
            target_launch_path = Some(paths::display_path(&launch.selected));
            let selected = launch
                .selected
                .to_str()
                .ok_or_else(|| AppError::Invalid("启动程序路径包含无法表示的字符".into()))?
                .to_owned();
            let evidence = if pin_executable {
                Some((
                    required_identity(root)?,
                    launch.program.clone(),
                    program_evidence(&launch.program)?,
                ))
            } else {
                None
            };
            (
                LaunchPathEdit::Set(path.to_owned()),
                Some(selected),
                evidence,
            )
        }
    };
    let state_sha256 = digest(&(
        &game.id,
        &game.root_path,
        &game.created_at,
        &game.name,
        &target_game_name,
        launch_path(&game),
        &launch_path_edit,
        &selected,
        &target_evidence,
        if renaming {
            game.metadata.get(AUTOMATIC_GAME_NAME)
        } else {
            None
        },
    ))?;
    let metadata = game
        .metadata
        .as_object_mut()
        .ok_or_else(|| AppError::Invalid("现有游戏 metadata 必须是 JSON 对象".into()))?;
    if let Some(selected) = selected {
        metadata.insert("launchPath".into(), selected.into());
    } else if matches!(launch_path_edit, LaunchPathEdit::Clear) {
        metadata.remove("launchPath");
    }
    if renaming {
        metadata.remove(AUTOMATIC_GAME_NAME);
    }
    let preview = UpdatePreview {
        game_id: game.id.clone(),
        game_name: game.name.clone(),
        root_path,
        target_game_name: target_game_name.clone(),
        current_launch_path,
        target_launch_path,
        changes_launch_path: !matches!(launch_path_edit, LaunchPathEdit::Keep),
        launch_path_edit,
        state_sha256,
    };
    game.name = target_game_name;
    Ok((preview, game))
}

pub(crate) fn prepare_removal(engine: &FileEngine, id: &str) -> AppResult<RemovalPreview> {
    let _guard = transaction::write_lock();
    review_removal(&engine.db.conn(), id)
}

pub(crate) fn apply_removal(engine: &FileEngine, approved: &RemovalPreview) -> AppResult<()> {
    commit_removal(engine, &approved.game_id, Some(approved))
}

pub(super) fn commit_removal(
    engine: &FileEngine,
    id: &str,
    approved: Option<&RemovalPreview>,
) -> AppResult<()> {
    let _guard = transaction::write_lock();
    let mut conn = engine.db.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = review_removal(&tx, id)?;
    if let Some(approved) = approved {
        require_same(&current, approved)?;
    }
    // Preserve the journal and related historical records. Revoking the game
    // registration only removes its live authorization and workspace membership.
    tx.execute("DELETE FROM game_user_roots WHERE game_id=?1", [id])?;
    crate::history::remove_game_from_workspaces(&tx, id)?;
    tx.execute("DELETE FROM games WHERE id=?1", [id])?;
    tx.commit()?;
    Ok(())
}

fn review_removal(conn: &Connection, id: &str) -> AppResult<RemovalPreview> {
    // Removal still works when the directory or opaque game metadata cannot
    // be read. It revokes a DB registration and never opens the disk target.
    let (name, root_path, created_at, slug, platform, adapter_slug, adapter_version) = conn
        .query_row(
            "SELECT name,root_path,created_at,slug,platform,adapter_slug,adapter_version FROM games WHERE id=?1",
            [id],
            |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            )),
        )
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("游戏: {id}")))?;
    let installed: i64 = conn.query_row(
        &format!(
            "SELECT count(*) FROM installs WHERE game_id=?1 AND (
          status='installed' OR (CASE WHEN EXISTS(
            SELECT 1 FROM install_transactions it WHERE it.install_id=installs.id
          ) THEN {INSTALL_STATE} ELSE status END) IN ('enabled','partially_restored'))"
        ),
        [id],
        |row| row.get(0),
    )?;
    if installed > 0 {
        return Err(AppError::Conflict(format!(
            "该游戏仍有 {installed} 个已安装的 Mod，请先在游戏详情中卸载后再移除"
        )));
    }
    // Recovery can depend on path authorizations even when a journal has no game_id.
    let needs_recovery: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM file_transactions
           WHERE status IN ('applying','rolling_back','recovery_failed'))",
        [],
        |row| row.get(0),
    )?;
    if needs_recovery {
        return Err(AppError::Conflict(
            "存在待恢复的文件事务，请先恢复后再移除游戏".into(),
        ));
    }
    if has_pending_review(conn, id, PathBuf::from(&root_path), GameChange::Remove)? {
        return Err(AppError::Conflict(
            "该游戏仍有待审批或待执行的计划，请先处理后再移除".into(),
        ));
    }

    let mut stmt =
        conn.prepare("SELECT root_id,path FROM game_user_roots WHERE game_id=?1 ORDER BY root_id")?;
    let user_roots = stmt
        .query_map([id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let state_sha256 = digest(&(
        id,
        &name,
        &root_path,
        &created_at,
        &slug,
        &platform,
        &adapter_slug,
        &adapter_version,
        &user_roots,
    ))?;
    Ok(RemovalPreview {
        game_id: id.to_owned(),
        game_name: name,
        root_path: paths::display_path(Path::new(&root_path)),
        user_roots: user_roots
            .into_iter()
            .map(|(root_id, path)| RemovalUserRoot {
                root_id,
                path: paths::display_path(Path::new(&path)),
            })
            .collect(),
        state_sha256,
    })
}

pub(crate) fn prepare_launch(engine: &FileEngine, id: &str) -> AppResult<LaunchPreview> {
    let _guard = transaction::write_lock();
    Ok(review_launch(engine, id)?.0)
}

pub(crate) fn apply_launch(engine: &FileEngine, approved: &LaunchPreview) -> AppResult<()> {
    apply_launch_with(engine, approved, dispatch_launch)
}

pub(super) fn dispatch_launch(action: LaunchAction) -> AppResult<()> {
    match action {
        LaunchAction::Program(program) => launch_program(&program, &[]),
        LaunchAction::Steam(url) => tauri_plugin_opener::open_url(url, None::<&str>)
            .map_err(|error| AppError::Other(format!("无法通过 Steam 启动游戏: {error}"))),
    }
}

pub(super) fn apply_launch_with(
    engine: &FileEngine,
    approved: &LaunchPreview,
    dispatch: impl FnOnce(LaunchAction) -> AppResult<()>,
) -> AppResult<()> {
    let _guard = transaction::write_lock();
    let (current, action) = review_launch(engine, &approved.game_id)?;
    require_same(&current, approved)?;
    dispatch(action)
}

fn review_launch(engine: &FileEngine, id: &str) -> AppResult<(LaunchPreview, LaunchAction)> {
    let game = games::get_game(&engine.db, id)?;
    let root = Path::new(&game.root_path);
    let action = decide_launch(&engine.paths, &game)?;
    let (target, target_evidence) = match &action {
        LaunchAction::Program(program) => {
            let canonical_root = checked_game_root(&engine.paths, root)?;
            (
                LaunchTarget::Program {
                    program_path: paths::display_path(program),
                },
                Some((
                    required_identity(&canonical_root)?,
                    program,
                    program_evidence(program)?,
                )),
            )
        }
        LaunchAction::Steam(url) => (LaunchTarget::Steam { url: url.clone() }, None),
    };
    let state_sha256 = digest(&(
        &game.id,
        &game.name,
        &game.root_path,
        &game.created_at,
        launch_path(&game),
        &target,
        &target_evidence,
    ))?;
    Ok((
        LaunchPreview {
            game_id: game.id.clone(),
            game_name: game.name.clone(),
            root_path: paths::display_path(root),
            launch_path: display_launch_path(&game),
            target,
            state_sha256,
        },
        action,
    ))
}
