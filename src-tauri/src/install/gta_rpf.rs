//! Native authority for the GTA V RPF job protocol. Nothing returned
//! by an MCP chooses a destination. Only this module constructs deployment ops.
use super::{
    gta_layout::{enhanced_rpf_unsupported, Edition},
    InstallInspection, InstallPreview, InstalledMod,
};
use crate::{
    error::{AppError, AppResult, NativeMessage},
    games, mod_library,
    mod_organization::OrganizationSuggestion,
    safe_fs,
    transaction::{self, FileEngine, FileOp, FilePlan, Fingerprint},
    util,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

#[path = "gta_rpf/archive.rs"]
mod archive;
#[path = "gta_rpf/files.rs"]
mod files;
#[path = "gta_rpf/journal.rs"]
mod journal;
#[path = "gta_rpf/model.rs"]
mod model;
pub(crate) use archive::{prepare_archive_job, MAX_ARCHIVE_PAYLOADS};
use files::*;
pub(crate) use journal::{
    active_files, guard_foreign_plan, inspect, plan_install, plan_uninstall, validate_apply,
    validate_rollback,
};
use model::*;
pub(crate) const KIND: &str = "gta_rpf";
pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS gta_rpf_jobs (id TEXT PRIMARY KEY, game_id TEXT NOT NULL, frozen TEXT NOT NULL); CREATE TABLE IF NOT EXISTS gta_rpf_changes (transaction_id TEXT PRIMARY KEY REFERENCES file_transactions(id), job_id TEXT NOT NULL UNIQUE REFERENCES gta_rpf_jobs(id), game_id TEXT NOT NULL, state TEXT NOT NULL, plan_sha TEXT NOT NULL); CREATE INDEX IF NOT EXISTS gta_rpf_changes_game ON gta_rpf_changes(game_id);";
fn invalid(s: &str) -> AppError {
    AppError::Invalid(s.into())
}
fn conflict(s: &str) -> AppError {
    AppError::Conflict(s.into())
}
fn check_cancel(c: &CancellationToken) -> AppResult<()> {
    if c.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreparedJob {
    pub job_id: String,
    pub game_id: String,
    pub action: &'static str,
}

fn check_edition_job(edition: Edition, kind: &str, menyoo: bool) -> AppResult<()> {
    if edition == Edition::Enhanced && (!edition.supports_rpf_kind(kind) || menyoo) {
        return Err(AppError::Presented(enhanced_rpf_unsupported()));
    }
    Ok(())
}

pub(crate) fn validate_job_options(
    slug: &str,
    kind: Option<&str>,
    options: Option<&Value>,
) -> AppResult<()> {
    if let (Some(edition), Some(kind)) = (Edition::from_slug(slug), kind) {
        let menyoo = options.and_then(|o| o["menyoo"].as_bool()).unwrap_or(false);
        check_edition_job(edition, kind, menyoo)?;
    }
    Ok(())
}

fn binding(e: &FileEngine, game: &str, c: &CancellationToken) -> AppResult<Binding> {
    binding_for_game(e, &games::get_game(&e.db, game)?, c)
}

fn checked_game_root(
    e: &FileEngine,
    g: &games::Game,
) -> AppResult<(Edition, PathBuf, crate::plugins::GameAdapter)> {
    let slug = g
        .adapter_slug
        .as_deref()
        .ok_or_else(|| invalid("请先绑定 GTA 适配包"))?;
    let edition = Edition::from_slug(slug).ok_or_else(|| invalid("请先绑定 GTA 适配包"))?;
    super::gta_layout::check_edition(g, slug)?;
    let adapter = crate::game_roots::adapter(&e.db, g)?;
    if adapter.slug != slug
        || g.adapter_version
            .as_deref()
            .is_some_and(|s| s != adapter.version)
    {
        return Err(conflict("GTA 适配包绑定版本已变化"));
    }
    let root = PathBuf::from(&g.root_path);
    e.paths.stable_directory(&root)?;
    if e.paths.overlaps_storage(&root) || crate::paths::overlaps(&root, &jobs(e)?) {
        return Err(invalid("游戏与 RPF 存储根重叠"));
    }
    Ok((edition, root, adapter))
}

fn binding_for_game(e: &FileEngine, g: &games::Game, c: &CancellationToken) -> AppResult<Binding> {
    let (edition, root, adapter) = checked_game_root(e, g)?;
    let marker = root.join(edition.marker());
    let original = root.join("update/update.rpf");
    e.stable_path(&marker, false)?;
    e.stable_path(&original, false)?;
    let exe = file_fp(&marker, c)?.ok_or_else(|| {
        AppError::Presented(NativeMessage::new(
            "gta.rpf.marker_missing",
            [("marker", edition.marker().into())],
        ))
    })?;
    let original_fp =
        file_fp(&original, c)?.ok_or_else(|| invalid("缺少原版 update/update.rpf"))?;
    check_shape(&original, "update")?;
    Ok(Binding {
        game: g.id.clone(),
        root,
        slug: adapter.slug,
        version: adapter.version,
        exe,
        original: original_fp,
    })
}
fn head(e: &FileEngine, game: &str) -> AppResult<(Option<String>, State)> {
    let row:Option<(String,String)>=e.db.conn().query_row("SELECT c.transaction_id,c.state FROM gta_rpf_changes c JOIN file_transactions f ON f.id=c.transaction_id WHERE c.game_id=?1 AND f.status='committed' AND f.kind='gta_rpf' ORDER BY c.rowid DESC LIMIT 1",[game],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    match row {
        None => Ok((None, State::default())),
        Some((id, s)) => Ok((Some(id), serde_json::from_str(&s)?)),
    }
}
fn frozen(e: &FileEngine, id: &str) -> AppResult<Frozen> {
    if !digest_ok(id, 32) {
        return Err(invalid("RPF jobId 必须是 32 位小写十六进制"));
    }
    let raw: String =
        e.db.conn()
            .query_row("SELECT frozen FROM gta_rpf_jobs WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or_else(|| invalid("未知本机 RPF 任务"))?;
    let j: Frozen = serde_json::from_str(&raw)?;
    if j.input.job_id != id || j.input.kind != j.binding.edition()? {
        return Err(conflict("RPF 任务账本不一致"));
    }
    check_edition_job(j.input.kind, j.item.contribution.kind(), j.item.menyoo)?;
    Ok(j)
}
fn pending(e: &FileEngine) -> AppResult<()> {
    if e.db.conn().query_row("SELECT EXISTS(SELECT 1 FROM file_transactions WHERE status IN ('applying','rolling_back','recovery_failed'))",[],|r|r.get::<_,bool>(0))?{return Err(conflict("存在待恢复的文件事务"));}
    Ok(())
}
fn check_active(
    e: &FileEngine,
    b: &Binding,
    state: &State,
    c: &CancellationToken,
) -> AppResult<()> {
    if state.binding.as_ref().is_some_and(|old| old != b) && !state.items.is_empty() {
        return Err(conflict("游戏版本、原始 RPF 或适配包与活跃基线不符"));
    }
    for (key, blob) in &state.files {
        let p = destination(&b.root, key)?;
        e.stable_path(&p, false)?;
        if file_fp(&p, c)? != Some(blob.fp()) {
            return Err(conflict("已托管的 RPF 或 Menyoo 文件内容发生漂移"));
        }
    }
    Ok(())
}
fn verify_archives(e: &FileEngine, b: &Binding, items: &[Item]) -> AppResult<()> {
    for i in items {
        let verified = mod_library::verify_for_install(e, &b.game, &i.artifact)?;
        if verified.entry.sha256 != i.digest {
            return Err(conflict("模组原包与冻结贡献不符"));
        }
    }
    Ok(())
}
fn ensure_inactive(e: &FileEngine, b: &Binding, item: &Item) -> AppResult<()> {
    let exists:bool=e.db.conn().query_row(&format!("SELECT EXISTS(SELECT 1 FROM installs WHERE game_id=?1 AND (json_extract(metadata_json,'$.libraryEntryId')=?2 OR json_extract(metadata_json,'$.downloadOrigin.artifactId')=?2) AND ({}) IN ('enabled','installed','partially_restored'))",super::effective_install_state()),params![b.game,item.artifact],|r|r.get(0))?;
    if exists {
        return Err(conflict("原包已安装"));
    }
    Ok(())
}

/// Must be called outside the FileEngine write lock. Public calls receive no
/// directory capability: engine roots and the bound artifact determine every path.
pub(crate) fn prepare_job(
    e: &FileEngine,
    game_id: &str,
    artifact_id: Option<&str>,
    install_id: Option<&str>,
    kind: Option<&str>,
    options: Option<&Value>,
    c: &CancellationToken,
) -> AppResult<PreparedJob> {
    let _guard = transaction::write_lock_cancellable(c)?;
    pending(e)?;
    let game = games::get_game(&e.db, game_id)?;
    if let Some(slug) = &game.adapter_slug {
        validate_job_options(slug, kind, options)?;
    }
    let b = binding_for_game(e, &game, c)?;
    let edition = b.edition()?;
    let (parent, state) = head(e, game_id)?;
    check_active(e, &b, &state, c)?;
    let mut consumed_probe = None;
    let (install, item) = match (artifact_id, install_id, kind) {
        (Some(a), None, Some(k)) => {
            let adapter = crate::game_roots::adapter(&e.db, &game)?;
            if !adapter.mod_types.iter().any(|t| t.id == format!("rpf-{k}")) {
                return Err(invalid("适配包未声明该 RPF Mod 类型"));
            }
            let (entry, members) = super::library::gta_payload(e, game_id, a, c)?;
            let verified = mod_library::verify_for_install(e, game_id, a)?;
            let single =
                members.len() == 1 && members.values().next() == Some(&verified.archive_path);
            let hashes = if single {
                BTreeMap::from([(PathBuf::from(&entry.filename), entry.sha256.clone())])
            } else {
                crate::archive::verify_archive_members(
                    &verified.archive_path,
                    &entry.sha256,
                    &BTreeMap::new(),
                    c,
                )?
                .hashes
            };
            let mut opts = options.cloned().unwrap_or_else(|| json!({}));
            let inquiry = opts
                .as_object_mut()
                .ok_or_else(|| invalid("options 必须是对象"))?
                .remove("inspectJobId");
            let mut item = make_item(k, Some(&opts), &entry, &members, |name, path| {
                let hash = hashes
                    .get(Path::new(name))
                    .ok_or_else(|| invalid("缺少原包成员指纹"))?;
                freeze(e, path, Some(hash), c)
            })?;
            ensure_inactive(e, &b, &item)?;
            if let Contribution::Dlc { source, .. } = &item.contribution {
                check_shape(&blob_path(e, source)?, "dlc")?;
            } else if let Contribution::Gameconfig { source } = &item.contribution {
                check_shape(&blob_path(e, source)?, "gameconfig")?;
            }
            if let Some(id) = inquiry {
                let id = id
                    .as_str()
                    .ok_or_else(|| invalid("inspectJobId 必须是字符串"))?;
                let probe = frozen(e, id)?;
                let Contribution::Dlc { pack_name, source } = &mut item.contribution else {
                    return Err(invalid("只有 DLC 可使用 inspectJobId"));
                };
                if !pack_name.is_empty()
                    || probe.input.inspect_dlc.as_ref() != Some(source)
                    || probe.binding != b
                    || probe.item.artifact != item.artifact
                    || probe.item.digest != item.digest
                {
                    return Err(conflict("DLC 探测任务与原包、选中的文件或游戏不符"));
                }
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct Probe {
                    schema_version: u8,
                    job_id: String,
                    status: String,
                    pack_name: String,
                }
                if small_file(&jobs(e)?.join(id).join("input.json"), 2 * 1024 * 1024)?
                    != serde_json::to_vec(&probe.input)?
                {
                    return Err(conflict("RPF 探测输入清单已变化"));
                }
                require_workspace(e, &probe)?;
                let receipt: Probe = serde_json::from_slice(&small_file(
                    &jobs(e)?.join(id).join("result.json"),
                    16 * 1024,
                )?)?;
                if receipt.job_id != id
                    || receipt.status != "inspected"
                    || receipt.schema_version != 1
                    || !identifier(&receipt.pack_name)
                    || receipt.pack_name.eq_ignore_ascii_case("gmm")
                {
                    return Err(invalid("DLC 内部 nameHash 无效"));
                }
                *pack_name = receipt.pack_name.to_ascii_lowercase();
                consumed_probe = Some(probe);
            }
            (true, item)
        }
        (None, Some(i), None)
            if options.is_none_or(|o| o.as_object().is_some_and(|m| m.is_empty())) =>
        {
            let item = state
                .items
                .iter()
                .find(|x| x.id == i)
                .ok_or_else(|| conflict("不是该游戏的活跃 RPF 贡献"))?
                .clone();
            (false, item)
        }
        _ => {
            return Err(invalid(
                "RPF 安装需要 artifactId 和 kind；卸载仅接受 installId",
            ))
        }
    };
    check_edition_job(edition, item.contribution.kind(), item.menyoo)?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut input = Input {
        schema_version: 1,
        job_id: id.clone(),
        kind: edition,
        targets: vec![],
        inspect_dlc: None,
    };
    if let Contribution::Dlc { source, pack_name } = &item.contribution {
        if pack_name.is_empty() {
            input.inspect_dlc = Some(source.clone());
        }
    }
    let mut bases = state.bases.clone();
    if input.inspect_dlc.is_none() {
        let next = after_items(&state, &item, install)?;
        verify_archives(e, &b, &state.items)?;
        for key in item.keys() {
            let p = destination(&b.root, &key)?;
            e.stable_path(&p, false)?;
            super::verify_unowned_paths(e, [p.as_path()])?;
            if !bases.contains_key(&key) {
                let current = file_fp(&p, c)?;
                if matches!(key.as_str(), "gmm") || key.starts_with("dlc:") {
                    if current.is_some() {
                        return Err(conflict("同名 DLC 或 gmm 已存在且不归 AMM 管理"));
                    }
                } else if key == "menyoo" && current.is_none() {
                    return Err(invalid("尚无 Menyoo 车辆列表，不自动安装或创建前置"));
                }
                let backup = current
                    .map(|fp| freeze(e, &p, Some(&fp.sha256), c))
                    .transpose()?;
                bases.insert(key.clone(), backup);
            }
            let contributors: Vec<_> = next
                .iter()
                .filter(|i| i.keys().contains(&key))
                .map(|i| i.contribution.clone())
                .collect();
            if contributors.is_empty() || key.starts_with("dlc:") {
                continue;
            }
            let baseline = match (&*key, &bases[&key]) {
                ("update", None) => Some(freeze(
                    e,
                    &b.root.join("update/update.rpf"),
                    Some(&b.original.sha256),
                    c,
                )?),
                (_, v) => v.clone(),
            };
            if let Some(bl) = &baseline {
                check_shape(&blob_path(e, bl)?, &key)?;
            }
            input.targets.push(WireTarget {
                id: key,
                baseline,
                contributions: contributors,
            });
        }
    }
    let probe = input.inspect_dlc.is_some();
    let (mut cleanup, identity) = write_job(e, &input, c)?;
    let frozen = Frozen {
        workspace: Some(identity),
        binding: b,
        parent,
        install,
        item,
        input,
        bases,
    };
    check_cancel(c)?;
    e.db.conn().execute(
        "INSERT INTO gta_rpf_jobs(id,game_id,frozen) VALUES (?1,?2,?3)",
        params![id, game_id, serde_json::to_string(&frozen)?],
    )?;
    cleanup.keep();
    if let Some(probe) = consumed_probe {
        clean_consumed(e, &probe);
    }
    Ok(PreparedJob {
        job_id: id,
        game_id: game_id.into(),
        action: if probe {
            "inspectDlc"
        } else if install {
            "install"
        } else {
            "uninstall"
        },
    })
}

// Real FileEngine workflows and directory-identity cleanup target Windows.
#[cfg(all(test, windows))]
#[path = "gta_rpf/tests.rs"]
mod tests;
