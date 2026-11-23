//! Game-library actions and path checks shared by desktop IPC, Agent conversations and MCP.
use crate::{
    error::{AppError, AppResult},
    game_roots,
    games::{self, Game},
    paths,
    plugins::{GameAdapter, Platform, RootKind},
    storage::AppPaths,
    transaction::{self, FileEngine, FileOp, FilePlan},
    util,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn validate_name(name: &str) -> AppResult<&str> {
    let name = name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err(AppError::Invalid(
            "游戏名称去除首尾空白后不能为空，且不能含有控制字符".into(),
        ));
    }
    Ok(name)
}

mod registration;
mod review;
pub(crate) use registration::{
    apply_registration, prepare_registration, register_local_from_desktop,
};
pub(crate) use review::{
    apply_launch, apply_removal, apply_update, launch_from_desktop, prepare_launch,
    prepare_removal, prepare_update,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RegistrationPreview {
    pub game_id: String,
    pub game_name: String,
    pub root_path: String,
    pub platform: String,
    pub adapter_slug: Option<String>,
    pub adapter_version: Option<String>,
    pub steam_app_id: Option<String>,
    requested_directory: String,
    state_sha256: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum LaunchPathUpdate<'a> {
    Keep,
    Clear,
    Set(&'a str),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "path")]
enum LaunchPathEdit {
    Keep,
    Clear,
    Set(String),
}

impl LaunchPathEdit {
    fn as_update(&self) -> LaunchPathUpdate<'_> {
        match self {
            Self::Keep => LaunchPathUpdate::Keep,
            Self::Clear => LaunchPathUpdate::Clear,
            Self::Set(path) => LaunchPathUpdate::Set(path),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UpdatePreview {
    pub game_id: String,
    pub game_name: String,
    pub root_path: String,
    pub target_game_name: String,
    pub current_launch_path: Option<String>,
    pub target_launch_path: Option<String>,
    pub changes_launch_path: bool,
    launch_path_edit: LaunchPathEdit,
    state_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemovalPreview {
    pub game_id: String,
    pub game_name: String,
    pub root_path: String,
    pub user_roots: Vec<RemovalUserRoot>,
    state_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemovalUserRoot {
    pub root_id: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub(crate) enum LaunchTarget {
    Program { program_path: String },
    Steam { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LaunchPreview {
    pub game_id: String,
    pub game_name: String,
    pub root_path: String,
    pub launch_path: Option<String>,
    pub target: LaunchTarget,
    state_sha256: String,
}

pub(crate) fn update_game(
    engine: &FileEngine,
    id: &str,
    name: &str,
    launch_path: Option<&str>,
) -> AppResult<Game> {
    let launch_path = launch_path
        .filter(|path| !path.trim().is_empty())
        .map_or(LaunchPathUpdate::Clear, LaunchPathUpdate::Set);
    review::commit_update(engine, id, Some(validate_name(name)?), launch_path, None)
}

/// The reviewed choice. Paths here are display paths; execution always uses
/// the registered canonical game root, which is covered by the state digest.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GameAdapterBinding {
    pub game_id: String,
    pub game_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_game_name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    uses_adapter_game_name: bool,
    pub root_path: String,
    pub current_adapter_slug: Option<String>,
    pub current_adapter_version: Option<String>,
    pub target_adapter_slug: String,
    pub target_adapter_name: String,
    pub target_adapter_version: String,
    pub matched_markers: Vec<String>,
    pub missing_required_markers: Vec<String>,
    state_sha256: String,
}

impl GameAdapterBinding {
    fn requested_name(&self) -> Option<&str> {
        self.target_game_name
            .as_deref()
            .filter(|_| !self.uses_adapter_game_name)
    }
}

pub(crate) fn set_game_adapter(engine: &FileEngine, id: &str, slug: &str) -> AppResult<Game> {
    commit_game_adapter_binding(engine, id, slug, None)
}

pub(crate) fn prepare_game_adapter_binding(
    engine: &FileEngine,
    game_id: &str,
    slug: &str,
    target_game_name: Option<&str>,
) -> AppResult<GameAdapterBinding> {
    let _guard = transaction::write_lock();
    let (root, selected) = inspect_registered_binding(engine, game_id, slug)?;
    let conn = engine.db.conn();
    let game = games::get_game_from(&conn, game_id)?;
    binding_preview(&conn, &game, &root, &selected, target_game_name)
}

pub(crate) fn apply_game_adapter_binding(
    engine: &FileEngine,
    binding: &GameAdapterBinding,
) -> AppResult<Game> {
    commit_game_adapter_binding(
        engine,
        &binding.game_id,
        &binding.target_adapter_slug,
        Some(binding),
    )
}

fn commit_game_adapter_binding(
    engine: &FileEngine,
    game_id: &str,
    slug: &str,
    approved: Option<&GameAdapterBinding>,
) -> AppResult<Game> {
    // Reconciliation has its own lock ordering and does not change game bindings.
    crate::plugin_workflow::reconcile_deployments(engine)?;
    let _guard = transaction::write_lock();
    let (root, selected) = inspect_registered_binding(engine, game_id, slug)?;
    let mut conn = engine.db.conn();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let game = games::get_game_from(&tx, game_id)?;
    let requested_name = approved.and_then(GameAdapterBinding::requested_name);
    let current = binding_preview(&tx, &game, &root, &selected, requested_name)?;
    if approved.is_some_and(|reviewed| *reviewed != current) {
        return Err(AppError::Conflict(
            "游戏登记、适配包或匹配结果已改变，请重新预览并审批绑定".into(),
        ));
    }
    // The same selection supplies both the reviewed effects and the mutation.
    // Its exact adapter and selected match, not just their display fields,
    // have been checked against the approval. No binding input is taken from args.
    let updated = games::rebind_game(&tx, game, &selected, current.requested_name())?;
    tx.commit()?;
    Ok(updated)
}

fn inspect_registered_binding(
    engine: &FileEngine,
    id: &str,
    slug: &str,
) -> AppResult<(PathBuf, games::GameAdapterSelection)> {
    let game = games::get_game(&engine.db, id)?;
    let root = checked_game_root(&engine.paths, Path::new(&game.root_path))?;
    if root.parent().is_none() {
        return Err(AppError::Invalid(
            "不能将整个文件系统根登记为游戏目录".into(),
        ));
    }
    let selected = games::inspect_game_adapter_selection(&engine.db, &root, slug)?;
    Ok((root, selected))
}

fn binding_preview(
    conn: &Connection,
    game: &Game,
    root: &Path,
    selected: &games::GameAdapterSelection,
    target_game_name: Option<&str>,
) -> AppResult<GameAdapterBinding> {
    let requested_name = target_game_name.map(validate_name).transpose()?;
    let automatic_name = game
        .automatic_adapter_name(&selected.adapter)
        .filter(|&name| name != game.name);
    let uses_adapter_game_name = requested_name.is_none() && automatic_name.is_some();
    let target_game_name = requested_name.or(automatic_name);
    if Path::new(&game.root_path) != root {
        return Err(AppError::Conflict("已登记的游戏目录已改变".into()));
    }
    if !game.metadata.is_object() {
        return Err(AppError::Invalid(
            "现有游戏 metadata 必须是 JSON 对象".into(),
        ));
    }
    check_adapter_change(
        conn,
        &game.id,
        root.to_owned(),
        "该游戏仍有待审批或待执行的计划，请处理后再选择适配包",
    )?;
    // Only binding-relevant state can stale approval. Unrelated metadata and
    // user-root edits are preserved; the conflict check still uses live roots.
    let mut state = serde_json::to_vec(&(
        &game.root_path,
        &game.created_at,
        &game.slug,
        &game.platform,
        &game.steam_app_id,
        game.confidence.to_bits(),
        &selected.adapter,
        &selected.matched,
    ))?;
    if let Some(target) = target_game_name {
        // The approval also binds whether the name stays eligible for automatic updates.
        serde_json::to_writer(&mut state, &(&game.name, target))?;
        if uses_adapter_game_name {
            serde_json::to_writer(&mut state, &uses_adapter_game_name)?;
        }
    }
    let state_sha256 = util::sha256_reader(&mut state.as_slice())?;
    Ok(GameAdapterBinding {
        game_id: game.id.clone(),
        game_name: game.name.clone(),
        target_game_name: target_game_name.map(str::to_owned),
        uses_adapter_game_name,
        root_path: paths::display_path(root),
        current_adapter_slug: game.adapter_slug.clone(),
        current_adapter_version: game.adapter_version.clone(),
        target_adapter_slug: selected.adapter.slug.clone(),
        target_adapter_name: selected.adapter.name.clone(),
        target_adapter_version: selected.adapter.version.clone(),
        matched_markers: selected.matched.matched_markers.clone(),
        missing_required_markers: selected.matched.missing_required_markers.clone(),
        state_sha256,
    })
}

pub(crate) fn remove_game_adapter(engine: &FileEngine, id: &str) -> AppResult<Game> {
    let _guard = transaction::write_lock();
    let mut conn = engine.db.conn();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let game = games::get_game_from(&tx, id)?;
    // This is a database-only change: use the recorded roots for conflict checks
    // even if the game directory or the old package is no longer available.
    check_adapter_change(
        &tx,
        id,
        PathBuf::from(&game.root_path),
        "该游戏仍有待审批或待执行的计划，请处理后再解绑适配包",
    )?;
    // Registered user roots belong to the game, not the binding. Keep them for
    // existing installations and historical restoration paths.
    let changed = tx.execute(
        "UPDATE games SET adapter_slug=NULL, adapter_version=NULL, updated_at=?1
         WHERE id=?2 AND (adapter_slug IS NOT NULL OR adapter_version IS NOT NULL)",
        params![util::now(), id],
    )?;
    let updated = if changed == 0 {
        game
    } else {
        games::get_game_from(&tx, id)?
    };
    tx.commit()?;
    Ok(updated)
}

fn check_adapter_change(
    conn: &Connection,
    id: &str,
    root: PathBuf,
    pending_review_message: &str,
) -> AppResult<()> {
    let recovery_pending: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM file_transactions WHERE status IN ('applying','rolling_back','recovery_failed'))",
        [],
        |row| row.get(0),
    )?;
    if recovery_pending {
        return Err(AppError::Conflict("请先处理待恢复的文件事务".into()));
    }
    if has_pending_review(conn, id, root, GameChange::SelectAdapter)? {
        return Err(AppError::Conflict(pending_review_message.into()));
    }
    Ok(())
}

pub(crate) fn remove_game(engine: &FileEngine, id: &str) -> AppResult<()> {
    review::commit_removal(engine, id, None)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameChange {
    #[default]
    SelectAdapter,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum ConflictKind {
    FilePlan,
    Approval,
    Recovery,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GamePlanConflict {
    id: String,
    kind: ConflictKind,
    status: String,
    title: String,
    summary: String,
    paths: Vec<String>,
    detail: Option<String>,
    plan_id: Option<String>,
    can_resolve: bool,
    resolution_hint: Option<String>,
}

struct ConflictTask {
    info: GamePlanConflict,
    target: ConflictTarget,
}

enum ConflictTarget {
    Plan,
    Approval {
        conversation_id: Option<String>,
        tool_call_id: Option<String>,
        running: bool,
    },
    Recovery,
    Unverified,
}

pub(crate) fn list_game_conflicts(
    engine: &FileEngine,
    game_id: &str,
    change: GameChange,
) -> AppResult<Vec<GamePlanConflict>> {
    let conn = engine.db.conn();
    let game = games::get_game_from(&conn, game_id)?;
    Ok(
        collect_conflicts(&conn, game_id, game.root_path.into(), change, true, None)?
            .into_iter()
            .map(|task| task.info)
            .collect(),
    )
}

pub(crate) fn resolve_game_conflict(
    engine: &FileEngine,
    game_id: &str,
    conflict_id: &str,
    change: GameChange,
    confirm_approved: bool,
    reject_live_approval: impl FnOnce(&str, &str) -> AppResult<()>,
) -> AppResult<()> {
    let _guard = transaction::write_lock();
    let mut conn = engine.db.conn();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let game = games::get_game_from(&tx, game_id)?;
    let mut matches = collect_conflicts(
        &tx,
        game_id,
        game.root_path.into(),
        change,
        true,
        Some(conflict_id),
    )?
    .into_iter();
    let task = matches
        .next()
        .ok_or_else(|| AppError::NotFound("冲突不属于此游戏、已处理或不存在".into()))?;
    if matches.next().is_some() {
        return Err(AppError::Conflict("冲突 ID 不唯一，无法安全解除".into()));
    }
    match task.target {
        ConflictTarget::Recovery => {
            return Err(AppError::Conflict(
                "待恢复的文件事务不能解除，请先恢复事务".into(),
            ));
        }
        ConflictTarget::Unverified => {
            return Err(AppError::Conflict(
                "无法核对文件计划的游戏归属，不能解除".into(),
            ));
        }
        ConflictTarget::Plan => {
            if task.info.status == "approved" && !confirm_approved {
                return Err(AppError::Conflict(
                    "计划已批准，必须明确确认后才能解除".into(),
                ));
            }
            // Use the existing terminal failure state: reconciliation already knows how
            // to release its reservations. Preserve the preview and record the reason.
            let changed = tx.execute(
                "UPDATE file_transactions SET status='failed', finished_at=?1, error=?2
                 WHERE id=?3 AND status=?4 AND status IN ('planned','approved')",
                params![
                    util::now(),
                    "用户解除文件计划，未执行文件操作",
                    conflict_id,
                    task.info.status
                ],
            )?;
            if changed != 1 {
                return Err(AppError::Conflict("计划状态已变化，请刷新后重试".into()));
            }
        }
        ConflictTarget::Approval {
            conversation_id,
            tool_call_id,
            running,
        } => {
            // The durable row alone cannot stop a live Agent waiter. Deliver a
            // rejection through the same one-shot response path before persisting it.
            // While the DB is locked a live approval not found in the response map
            // may be initializing or have already received a response; don't race it.
            let response = conversation_id
                .as_deref()
                .map(|conv| reject_live_approval(conv, conflict_id));
            match response {
                Some(Ok(())) => {}
                Some(Err(AppError::NotFound(_))) | None if !running => {}
                Some(Err(AppError::NotFound(_))) | None => {
                    return Err(AppError::Conflict(
                        "审批正在初始化或已收到答复，请刷新后重试".into(),
                    ));
                }
                Some(Err(error)) => return Err(error),
            }
            let changed = tx.execute(
                "UPDATE approvals SET status='rejected', resolved_at=?1 WHERE id=?2 AND status='pending'",
                params![util::now(), conflict_id],
            )?;
            if changed != 1 {
                return Err(AppError::Conflict("审批状态已变化，请刷新后重试".into()));
            }
            if let Some(call) = tool_call_id {
                tx.execute(
                    "UPDATE tool_calls SET approval_status='rejected' WHERE id=?1 AND approval_status='pending'",
                    [call],
                )?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

fn has_pending_review(
    conn: &Connection,
    id: &str,
    root: PathBuf,
    change: GameChange,
) -> AppResult<bool> {
    let tasks = collect_conflicts(conn, id, root, change, false, None)?;
    if !tasks.is_empty()
        && tasks
            .iter()
            .all(|task| matches!(task.target, ConflictTarget::Unverified))
    {
        return Err(AppError::Conflict(
            "存在无法核对归属的未完成文件计划，请先处理该计划".into(),
        ));
    }
    Ok(!tasks.is_empty())
}

// A snapshot or trash record can bind a transaction without setting f.game_id.
const SCOPED_PLANS: &str = "WITH linked_plans(id) AS (
    SELECT st.transaction_id FROM snapshots s JOIN snapshot_transactions st ON st.snapshot_id=s.id
     WHERE s.game_id=?1
    UNION ALL SELECT t.transaction_id FROM trash_items t WHERE t.game_id=?1 AND t.transaction_id IS NOT NULL
  ), scoped_plans AS (
    SELECT f.*, (coalesce(f.game_id=?1,0) OR f.id IN (SELECT id FROM linked_plans)) AS explicit
    FROM file_transactions f
  )";

fn collect_conflicts(
    conn: &Connection,
    game_id: &str,
    root: PathBuf,
    change: GameChange,
    include_recovery: bool,
    conflict_id: Option<&str>,
) -> AppResult<Vec<ConflictTask>> {
    let mut roots = vec![root];
    let mut stmt = conn.prepare("SELECT path FROM game_user_roots WHERE game_id=?1")?;
    for path in stmt.query_map([game_id], |row| row.get::<_, String>(0))? {
        roots.push(path?.into());
    }
    let mut tasks = Vec::new();

    // Adapter changes preserve roots. Unapproved standalone file previews do
    // not depend on the adapter, but do block removal of the registration.
    let include_file_previews = matches!(change, GameChange::Remove);
    let mut stmt = conn.prepare(&format!(
        "{SCOPED_PLANS}
         SELECT id, status, kind, summary, explicit, plan_json, error FROM scoped_plans
          WHERE (?4 IS NULL OR id=?4) AND (
            (status IN ('planned','approved') AND (explicit OR
              (game_id IS NULL AND (?2 OR status='approved' OR kind NOT IN ('files','local_agent_files')))))
            OR (?3 AND status IN ('applying','rolling_back','recovery_failed')))
          ORDER BY created_at DESC, id"
    ))?;
    let mut rows = stmt.query(params![
        game_id,
        include_file_previews,
        include_recovery,
        conflict_id
    ])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let status: String = row.get(1)?;
        let kind: String = row.get(2)?;
        let summary: String = row.get(3)?;
        let explicit: bool = row.get(4)?;
        let json: String = row.get(5)?;
        let recovery = is_recovery(&status);
        let plan = stored_plan(&json, &id);
        let paths = plan_paths(plan.as_ref());
        let associated = explicit || overlaps_game(&paths, &roots);
        if !recovery && !associated && plan.is_some() {
            continue;
        }
        let (target, resolution_hint) = if recovery {
            (
                ConflictTarget::Recovery,
                Some("请先恢复事务；游戏库不能解除待恢复的事务。"),
            )
        } else if !associated {
            (
                ConflictTarget::Unverified,
                Some("无法读取计划并核对游戏归属，不能安全解除。"),
            )
        } else if status == "approved" {
            (
                ConflictTarget::Plan,
                Some("此计划已批准，须明确确认后才能解除。"),
            )
        } else {
            (ConflictTarget::Plan, None)
        };
        tasks.push(ConflictTask {
            info: GamePlanConflict {
                plan_id: Some(id.clone()),
                id,
                kind: if recovery {
                    ConflictKind::Recovery
                } else {
                    ConflictKind::FilePlan
                },
                status,
                title: if recovery {
                    "待恢复的文件事务"
                } else {
                    "文件计划"
                }
                .into(),
                summary: if recovery && !associated {
                    match change {
                        GameChange::SelectAdapter => {
                            "其他文件事务的恢复尚未完成，暂时不能更改适配包绑定。"
                        }
                        GameChange::Remove => "其他文件事务的恢复尚未完成，暂时不能移除游戏。",
                    }
                    .into()
                } else {
                    summary
                },
                paths: if associated {
                    display_plan_paths(&paths)
                } else {
                    Vec::new()
                },
                detail: if recovery && !associated {
                    None
                } else {
                    plan_detail(row.get(6)?, plan.as_ref())
                        .or_else(|| Some(format!("计划类型：{kind}")))
                },
                can_resolve: matches!(target, ConflictTarget::Plan),
                resolution_hint: resolution_hint.map(str::to_owned),
            },
            target,
        });
    }

    let mut stmt = conn.prepare(&format!(
        "{SCOPED_PLANS}
         SELECT a.id, a.title, a.detail, a.conversation_id, a.tool_call_id,
                coalesce(t.status='running',0), f.id, f.status, f.summary, f.plan_json,
                coalesce(c.game_id=?1 OR CASE WHEN json_valid(t.arguments_json) THEN
                  (json_extract(t.arguments_json,'$.game_id')=?1 OR
                   json_extract(t.arguments_json,'$.gameId')=?1 OR
                   json_extract(t.arguments_json,'$.request.gameId')=?1) END,0)
                  OR coalesce(f.explicit,0)
         FROM approvals a
         LEFT JOIN conversations c ON c.id=a.conversation_id
         LEFT JOIN tool_calls t ON t.id=a.tool_call_id
         LEFT JOIN scoped_plans f ON f.id=CASE WHEN json_valid(a.payload_json) THEN coalesce(
                json_extract(a.payload_json,'$.originalPlan.id'),
                json_extract(a.payload_json,'$.plan.id'),
                json_extract(a.payload_json,'$.id')) END
         WHERE a.status='pending' AND (?2 IS NULL OR a.id=?2)
         ORDER BY a.created_at DESC, a.id"
    ))?;
    let mut rows = stmt.query(params![game_id, conflict_id])?;
    while let Some(row) = rows.next()? {
        let explicit: bool = row.get(10)?;
        let plan_id: Option<String> = row.get(6)?;
        let json: Option<String> = row.get(9)?;
        let plan = json
            .as_deref()
            .zip(plan_id.as_deref())
            .and_then(|(json, id)| stored_plan(json, id));
        let paths = plan_paths(plan.as_ref());
        let associated = explicit || overlaps_game(&paths, &roots);
        if !associated && (plan_id.is_none() || plan.is_some()) {
            continue;
        }
        let recovering = row
            .get::<_, Option<String>>(7)?
            .is_some_and(|status| is_recovery(&status));
        let target = if recovering {
            ConflictTarget::Recovery
        } else if !associated {
            ConflictTarget::Unverified
        } else {
            ConflictTarget::Approval {
                conversation_id: row.get(3)?,
                tool_call_id: row.get(4)?,
                running: row.get(5)?,
            }
        };
        let title: String = row.get(1)?;
        tasks.push(ConflictTask {
            info: GamePlanConflict {
                id: row.get(0)?,
                kind: ConflictKind::Approval,
                status: "pending".into(),
                summary: row
                    .get::<_, Option<String>>(8)?
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| title.clone()),
                title,
                paths: display_plan_paths(&paths),
                detail: plan_detail(row.get(2)?, plan.as_ref()),
                plan_id,
                can_resolve: matches!(target, ConflictTarget::Approval { .. }),
                resolution_hint: Some(
                    match target {
                        ConflictTarget::Recovery => "关联的文件事务尚待恢复，不能在此解除。",
                        ConflictTarget::Unverified => "无法核对关联计划的游戏归属，不能安全解除。",
                        _ => "解除会拒绝审批，不会执行计划或回滚文件。",
                    }
                    .into(),
                ),
            },
            target,
        });
    }
    Ok(tasks)
}

fn is_recovery(status: &str) -> bool {
    matches!(status, "applying" | "rolling_back" | "recovery_failed")
}

fn stored_plan(json: &str, id: &str) -> Option<FilePlan> {
    serde_json::from_str::<FilePlan>(json)
        .ok()
        .filter(|plan| plan.id == id)
}

fn plan_detail(detail: Option<String>, plan: Option<&FilePlan>) -> Option<String> {
    let detail = detail.filter(|s| !s.is_empty());
    let mut operations = BTreeMap::new();
    for entry in plan.into_iter().flat_map(|plan| &plan.entries) {
        let label = match &entry.operation {
            FileOp::Copy { .. } => "复制 (copy)",
            FileOp::Move { .. } => "移动 (move)",
            FileOp::Mkdir { .. } => "新建目录 (mkdir)",
            FileOp::WriteText { .. } => "写入文本 (write_text)",
            FileOp::Compress { .. } => "压缩文件夹 (compress)",
            FileOp::Extract { .. } => "解压归档 (extract)",
        };
        *operations.entry(label).or_insert(0) += 1;
    }
    if operations.is_empty() {
        return detail;
    }
    let operations = operations
        .into_iter()
        .map(|(label, count)| format!("{label} ×{count}"))
        .collect::<Vec<_>>()
        .join("、");
    Some(match detail {
        Some(detail) => format!("{detail}；计划操作：{operations}"),
        None => format!("计划操作：{operations}"),
    })
}

fn plan_paths(plan: Option<&FilePlan>) -> BTreeSet<&Path> {
    plan.into_iter()
        .flat_map(|plan| &plan.entries)
        .flat_map(|entry| [Some(entry.operation.dst()), entry.operation.src()])
        .flatten()
        .collect()
}

fn display_plan_paths(paths: &BTreeSet<&Path>) -> Vec<String> {
    paths.iter().map(|path| paths::display_path(path)).collect()
}

/// Plans can outlive their files. Compare their recorded components without
/// resolving today's links or folding names on case-sensitive volumes.
fn recorded_contains(root: &Path, path: &Path) -> bool {
    if path.starts_with(root) {
        return true;
    }
    #[cfg(windows)]
    {
        if path
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        {
            return false;
        }
        let root = paths::user_path(root);
        // Only shorten the ancestor being compared. A verbatim-only leaf can
        // still be below a safely representable registered root.
        path.ancestors()
            .any(|ancestor| paths::user_path(ancestor) == root)
    }
    #[cfg(not(windows))]
    false
}

fn overlaps_game(paths: &BTreeSet<&Path>, roots: &[PathBuf]) -> bool {
    paths.iter().any(|path| {
        path.is_absolute()
            && roots.iter().any(|root| {
                root.is_absolute()
                    && (recorded_contains(root, path) || recorded_contains(path, root))
            })
    })
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
                return Err(AppError::Denied(format!("请先添加用户管理目录 {root_id}")));
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
