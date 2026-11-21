//! Desktop library actions and their path checks. File operations stay in FileEngine.
use crate::{
    error::{AppError, AppResult},
    game_roots,
    games::{self, Game},
    install::INSTALL_STATE,
    plugins::{GameAdapter, Platform, RootKind},
    storage::AppPaths,
    transaction::{self, FileEngine, FilePlan},
    util,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn validate_name(name: &str) -> AppResult<&str> {
    let name = name.trim();
    if !(1..=80).contains(&name.chars().count()) || name.chars().any(char::is_control) {
        return Err(AppError::Invalid(
            "游戏名称去除首尾空白后须为 1 到 80 个字符，且不能含有控制字符".into(),
        ));
    }
    Ok(name)
}

pub(crate) fn update_game(
    engine: &FileEngine,
    id: &str,
    name: &str,
    launch_path: Option<&str>,
) -> AppResult<Game> {
    let name = validate_name(name)?;
    let _guard = transaction::write_lock();
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    let mut game = games::get_game_from(&tx, id)?;
    let root = Path::new(&game.root_path);
    let launch = launch_path
        .filter(|path| !path.trim().is_empty())
        .map(|path| validate_launch_path(&engine.paths, root, Path::new(path)))
        .transpose()?;
    let metadata = game
        .metadata
        .as_object_mut()
        .ok_or_else(|| AppError::Invalid("现有游戏 metadata 必须是 JSON 对象".into()))?;
    if let Some(launch) = launch {
        let path = launch
            .selected
            .to_str()
            .ok_or_else(|| AppError::Invalid("启动程序路径包含无法表示的字符".into()))?;
        metadata.insert("launchPath".into(), path.into());
    } else {
        metadata.remove("launchPath");
    }
    game.name = name.to_owned();
    game.updated_at = util::now();
    tx.execute(
        "UPDATE games SET name=?1,metadata_json=?2,updated_at=?3 WHERE id=?4",
        params![game.name, game.metadata.to_string(), game.updated_at, id],
    )?;
    tx.commit()?;
    Ok(game)
}

pub(crate) fn set_game_adapter(engine: &FileEngine, id: &str, slug: &str) -> AppResult<Game> {
    crate::plugin_workflow::reconcile_deployments(engine)?;
    let _guard = transaction::write_lock();
    let game = games::get_game(&engine.db, id)?;
    let root = checked_game_root(&engine.paths, Path::new(&game.root_path))?;
    let conn = engine.db.conn();
    let recovery_pending: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM file_transactions WHERE status IN ('applying','rolling_back','recovery_failed'))",
        [],
        |row| row.get(0),
    )?;
    if recovery_pending {
        return Err(AppError::Conflict("请先处理待恢复的文件事务".into()));
    }
    if has_pending_review(&conn, id, root, GameChange::SelectAdapter)? {
        return Err(AppError::Conflict(
            "该游戏仍有待审批或待执行的计划，请处理后再选择适配包".into(),
        ));
    }
    drop(conn);
    games::rebind_game(&engine.db, &game, slug)
}

pub(crate) fn remove_game(engine: &FileEngine, id: &str) -> AppResult<()> {
    let _guard = transaction::write_lock();
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    let root_path: String = tx
        .query_row("SELECT root_path FROM games WHERE id=?1", [id], |row| {
            row.get(0)
        })
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("游戏: {id}")))?;
    let installed: i64 = tx.query_row(
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
    let needs_recovery: bool = tx.query_row(
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
    if has_pending_review(&tx, id, PathBuf::from(root_path), GameChange::Remove)? {
        return Err(AppError::Conflict(
            "该游戏仍有待审批或待执行的计划，请先处理后再移除".into(),
        ));
    }

    // Preserve the journal and related historical records. A user root grants
    // access only through a live registration keyed by (game_id, root_id).
    tx.execute("DELETE FROM game_user_roots WHERE game_id=?1", [id])?;
    tx.execute(
        "UPDATE conversations SET game_id=NULL WHERE game_id=?1",
        [id],
    )?;
    tx.execute("DELETE FROM games WHERE id=?1", [id])?;
    tx.commit()?;
    Ok(())
}

enum GameChange {
    SelectAdapter,
    Remove,
}

fn has_pending_review(
    conn: &Connection,
    id: &str,
    root: PathBuf,
    change: GameChange,
) -> AppResult<bool> {
    let explicit: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM file_transactions f
           WHERE f.status IN ('planned','approved') AND (
             f.game_id=?1
             OR EXISTS(SELECT 1 FROM snapshots s JOIN snapshot_transactions st ON st.snapshot_id=s.id
                       WHERE s.game_id=?1 AND st.transaction_id=f.id)
             OR EXISTS(SELECT 1 FROM trash_items t WHERE t.game_id=?1 AND t.transaction_id=f.id)))
         OR EXISTS(SELECT 1 FROM approvals a
           LEFT JOIN conversations c ON c.id=a.conversation_id
           LEFT JOIN tool_calls t ON t.id=a.tool_call_id
           WHERE a.status='pending' AND
             (c.game_id=?1 OR (json_valid(t.arguments_json) AND
               (json_extract(t.arguments_json,'$.game_id')=?1 OR
                json_extract(t.arguments_json,'$.gameId')=?1 OR
                json_extract(t.arguments_json,'$.request.gameId')=?1))))",
        [id],
        |row| row.get(0),
    )?;
    if explicit {
        return Ok(true);
    }

    // Generic previews (including trash and snapshot plans) carry paths but
    // may have no game_id. Compare both endpoints with the registered roots.
    let mut roots = vec![root];
    let mut stmt = conn.prepare("SELECT path FROM game_user_roots WHERE game_id=?1")?;
    for path in stmt.query_map([id], |row| row.get::<_, String>(0))? {
        roots.push(path?.into());
    }
    let overlaps_root = |path: &Path| {
        roots
            .iter()
            .any(|root| path.starts_with(root) || root.starts_with(path))
    };
    // Selecting an adapter does not revoke any root. A standalone file preview
    // uses fixed paths, not adapter rules, and cannot run without approval.
    // Removal *does* revoke roots, so even those previews must block it.
    let include_file_previews = matches!(change, GameChange::Remove);
    let mut stmt = conn.prepare(
        "SELECT plan_json FROM file_transactions
           WHERE game_id IS NULL AND status IN ('planned','approved')
             AND (?1 OR status='approved' OR kind NOT IN ('files','local_agent_files'))
         UNION ALL
         SELECT f.plan_json FROM approvals a JOIN file_transactions f
           ON f.id=CASE WHEN json_valid(a.payload_json) THEN coalesce(
               json_extract(a.payload_json,'$.originalPlan.id'),
               json_extract(a.payload_json,'$.plan.id'),
               json_extract(a.payload_json,'$.id')) END
           WHERE a.status='pending'",
    )?;
    for json in stmt.query_map([include_file_previews], |row| row.get::<_, String>(0))? {
        let plan: FilePlan = serde_json::from_str(&json?).map_err(|_| {
            AppError::Conflict("存在无法核对归属的未完成文件计划，请先处理该计划".into())
        })?;
        if plan.entries.iter().any(|entry| {
            overlaps_root(entry.operation.dst()) || entry.operation.src().is_some_and(overlaps_root)
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn checked_game_root(paths: &AppPaths, root: &Path) -> AppResult<PathBuf> {
    let metadata = fs::symlink_metadata(root).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            AppError::NotFound("已登记的游戏根目录不存在".into())
        } else {
            AppError::Other(format!("无法读取已登记的游戏根目录: {error}"))
        }
    })?;
    let canonical = root
        .canonicalize()
        .map_err(|error| AppError::Other(format!("无法解析已登记的游戏根目录: {error}")))?;
    if !metadata.is_dir() || canonical != root {
        return Err(AppError::Denied(
            "已登记的游戏根目录已被替换，请核对原登记目录".into(),
        ));
    }
    let guarded = paths.guard(root, std::slice::from_ref(&canonical))?;
    if guarded != canonical {
        return Err(AppError::Denied(
            "已登记的游戏根目录已被替换，请核对原登记目录".into(),
        ));
    }
    Ok(guarded)
}

pub(crate) fn resolve_location(engine: &FileEngine, id: &str, target: &str) -> AppResult<PathBuf> {
    if !matches!(target, "root" | "mods") {
        return Err(AppError::Invalid(
            "打开目录的目标只能是 root 或 mods".into(),
        ));
    }
    let game = games::get_game(&engine.db, id)?;
    let root = Path::new(&game.root_path);
    if target == "root" {
        return checked_game_root(&engine.paths, root);
    }
    let adapter = game_roots::adapter(&engine.db, &game)?;
    let platform = game_roots::platform(&game)?;
    let registered = game_roots::registered_user_roots(&engine.db, id)?;
    resolve_mods_directory(&engine.paths, root, &adapter, platform, &registered)
}

fn resolve_mods_directory(
    paths: &AppPaths,
    root: &Path,
    adapter: &GameAdapter,
    platform: Platform,
    registered: &BTreeMap<String, PathBuf>,
) -> AppResult<PathBuf> {
    let game_roots: Vec<_> = std::iter::once(root.to_path_buf())
        .chain(registered.values().cloned())
        .collect();
    let managed_roots = paths.managed_roots(&game_roots);
    let mut failures = BTreeMap::new();
    for mod_type in &adapter.mod_types {
        let root_id = &mod_type.root;
        if failures.contains_key(root_id) {
            continue;
        }
        let result = (|| {
            let install_root = adapter
                .install_roots
                .iter()
                .find(|root| root.id == *root_id)
                .ok_or_else(|| AppError::NotFound(format!("适配包未声明安装根 {root_id}")))?;
            if install_root.platforms.contains(&platform)
                && install_root.kind == RootKind::RegisteredUser
                && !registered.contains_key(root_id)
            {
                return Err(AppError::Denied(format!("请先登记用户管理根 {root_id}")));
            }
            let directory = adapter
                .resolve_root(root_id, platform, root, registered, &managed_roots)
                .map_err(|error| match error {
                    AppError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        AppError::NotFound(format!("已登记的 Mod 管理根 {root_id} 不存在"))
                    }
                    error => error,
                })?;
            let guarded = paths.guard(&directory, &game_roots)?;
            if guarded != directory {
                return Err(AppError::Denied(format!(
                    "Mod 目录 {root_id} 在校验期间已变化"
                )));
            }
            if !guarded.is_dir() {
                return Err(AppError::NotFound(format!(
                    "Mod 目录不存在或不是目录: {}",
                    guarded.display()
                )));
            }
            Ok(guarded)
        })();
        match result {
            Ok(directory) => return Ok(directory),
            Err(error) => {
                failures.insert(root_id, error);
            }
        }
    }
    if failures.is_empty() {
        return Err(AppError::NotFound("该游戏的适配包未声明 Mod 目录".into()));
    }
    let details = failures
        .values()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("；");
    Err(AppError::Other(format!("无法定位 Mod 目录：{details}")))
}

struct ValidatedLaunchPath {
    selected: PathBuf,
    program: PathBuf,
}

fn guarded_launch_path(paths: &AppPaths, root: &Path, path: &Path) -> AppResult<PathBuf> {
    let canonical = path.canonicalize().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            AppError::NotFound("启动程序不存在，请在“编辑”中重新选择".into())
        } else {
            AppError::Other(format!("无法读取启动程序: {error}"))
        }
    })?;
    if !canonical.starts_with(root) {
        return Err(AppError::PathEscape(
            "启动程序必须位于该游戏的已登记根目录内".into(),
        ));
    }
    let guarded = paths.guard(&canonical, &[root.to_path_buf()])?;
    if guarded != canonical {
        return Err(AppError::Denied("启动程序路径在校验期间已变化".into()));
    }
    Ok(guarded)
}

fn validate_launch_path(
    paths: &AppPaths,
    root: &Path,
    path: &Path,
) -> AppResult<ValidatedLaunchPath> {
    if !path.is_absolute() {
        return Err(AppError::Invalid("启动程序必须使用绝对路径".into()));
    }
    let root = checked_game_root(paths, root)?;
    let selected = guarded_launch_path(paths, &root, path)?;
    #[cfg(target_os = "macos")]
    let program = if selected.is_dir() && has_extension(&selected, "app") {
        bundle_executable(paths, &root, &selected)?
    } else {
        selected.clone()
    };
    #[cfg(not(target_os = "macos"))]
    let program = selected.clone();
    let metadata = fs::symlink_metadata(&program)
        .map_err(|error| AppError::Other(format!("无法读取启动程序: {error}")))?;
    if !metadata.is_file() {
        return Err(AppError::Invalid("启动程序须为常规文件".into()));
    }
    #[cfg(windows)]
    if !has_extension(&program, "exe") {
        return Err(AppError::Invalid("Windows 启动程序必须是 .exe 文件".into()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(AppError::Invalid("启动程序没有可执行权限".into()));
        }
    }
    Ok(ValidatedLaunchPath { selected, program })
}

#[cfg(any(windows, target_os = "macos"))]
fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
}

#[cfg(target_os = "macos")]
fn bundle_executable(paths: &AppPaths, root: &Path, bundle: &Path) -> AppResult<PathBuf> {
    use std::io::Read;
    let invalid_bundle =
        || AppError::Invalid("macOS .app 应用缺少有效的 Info.plist 或主程序".into());
    let info = guarded_launch_path(paths, root, &bundle.join("Contents/Info.plist"))?;
    if !info.starts_with(bundle) {
        return Err(invalid_bundle());
    }
    let mut bytes = Vec::new();
    const MAX_PLIST: u64 = 1024 * 1024;
    crate::safe_fs::open_regular(&info)?
        .take(MAX_PLIST + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PLIST {
        return Err(invalid_bundle());
    }
    let value =
        plist::Value::from_reader(std::io::Cursor::new(bytes)).map_err(|_| invalid_bundle())?;
    let executable = value
        .as_dictionary()
        .and_then(|dict| dict.get("CFBundleExecutable"))
        .and_then(plist::Value::as_string)
        .filter(|name| {
            Path::new(name)
                .file_name()
                .is_some_and(|file_name| file_name == *name)
        })
        .ok_or_else(invalid_bundle)?;
    let program =
        guarded_launch_path(paths, root, &bundle.join("Contents/MacOS").join(executable))?;
    if !program.starts_with(bundle) {
        return Err(invalid_bundle());
    }
    Ok(program)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LaunchAction {
    Program(PathBuf),
    Steam(String),
}

pub(crate) fn decide_launch(paths: &AppPaths, game: &Game) -> AppResult<LaunchAction> {
    match game.metadata.get("launchPath") {
        Some(serde_json::Value::String(path)) if !path.trim().is_empty() => {
            let launch = validate_launch_path(paths, Path::new(&game.root_path), Path::new(path))?;
            return Ok(LaunchAction::Program(launch.program));
        }
        None | Some(serde_json::Value::Null | serde_json::Value::String(_)) => {}
        Some(_) => {
            return Err(AppError::Invalid(
                "已保存的启动程序路径无效，请在“编辑”中重新选择".into(),
            ));
        }
    }
    if let Some(id) = &game.steam_app_id {
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(AppError::Invalid("Steam App ID 必须为纯数字".into()));
        }
        return Ok(LaunchAction::Steam(format!("steam://rungameid/{id}")));
    }
    Err(AppError::Other(
        "尚未设置启动方式，请在“编辑”中选择游戏可执行文件".into(),
    ))
}

pub(crate) fn launch_program(program: &Path) -> AppResult<()> {
    let directory = program
        .parent()
        .ok_or_else(|| AppError::Invalid("无法确定启动程序所在目录".into()))?;
    let mut command = Command::new(program);
    command
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    spawn_program(command).map_err(|error| AppError::Other(format!("无法启动游戏: {error}")))
}

#[cfg(windows)]
fn spawn_program(mut command: Command) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
    command
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()?;
    Ok(())
}

#[cfg(unix)]
fn spawn_program(mut command: Command) -> std::io::Result<()> {
    use std::{
        ffi::CString,
        io,
        os::unix::{ffi::OsStrExt, process::CommandExt},
        sync::mpsc,
    };
    let path = CString::new(command.get_program().as_bytes())?;
    // Command's execvp fallback may invoke a shell for an unrecognized file.
    // SAFETY: execv is async-signal-safe. The owned CString and the terminated
    // argv stay valid during the call, and the closure does no allocation.
    unsafe {
        command.pre_exec(move || {
            let argv = [path.as_ptr(), std::ptr::null()];
            libc::execv(path.as_ptr(), argv.as_ptr());
            Err(io::Error::last_os_error())
        });
    }
    let (started, result) = mpsc::channel();
    // Create the waiter first, so its creation cannot strand a spawned child.
    std::thread::Builder::new().spawn(move || match command.spawn() {
        Ok(mut child) => {
            let _ = started.send(Ok(()));
            let _ = child.wait();
        }
        Err(error) => {
            let _ = started.send(Err(error));
        }
    })?;
    result.recv().map_err(io::Error::other)?
}

#[cfg(test)]
#[path = "game_actions_tests.rs"]
mod tests;
