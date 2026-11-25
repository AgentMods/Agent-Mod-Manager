use super::*;

fn plan_hash(p: &FilePlan) -> AppResult<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(p)?)))
}
#[derive(Default)]
struct Protection {
    files: BTreeSet<String>,
    artifacts: BTreeSet<(String, String)>,
}
/// Only *committed* generations own files. Rolled-back, planned and failed
/// generations never become the head. All decisions run inside WRITE_LOCK.
fn protected(e: &FileEngine, except: Option<&str>) -> AppResult<Protection> {
    let conn = e.db.conn();
    let mut st=conn.prepare("SELECT state FROM gta_rpf_changes c JOIN file_transactions f ON c.transaction_id=f.id WHERE f.status='committed' AND c.game_id IS NOT ?1 AND c.rowid=(SELECT max(c2.rowid) FROM gta_rpf_changes c2 JOIN file_transactions f2 ON c2.transaction_id=f2.id WHERE c2.game_id=c.game_id AND f2.status='committed')")?;
    let mut claims = Protection::default();
    // Never modify the vendor archive, even if AMM has not deployed its
    // first contribution (or the last one was just removed).
    let mut roots = conn.prepare("SELECT root_path FROM games WHERE id IS NOT ?1 AND adapter_slug IN ('gta5-legacy','gta5-enhanced')")?;
    for row in roots.query_map([except], |r| r.get::<_, String>(0))? {
        claims.files.insert(super::super::path_key(
            &PathBuf::from(row?).join("update/update.rpf"),
        ));
    }
    for raw in st.query_map([except], |r| r.get::<_, String>(0))? {
        let s: State = serde_json::from_str(&raw?)?;
        if !s.items.is_empty() {
            let binding = s.binding.ok_or_else(|| conflict("RPF 状态缺少绑定"))?;
            let root = &binding.root;
            if except.is_none() {
                claims.artifacts.extend(
                    s.items
                        .into_iter()
                        .map(|i| (binding.game.clone(), i.artifact)),
                );
            }
            for key in s.files.keys() {
                claims
                    .files
                    .insert(super::super::path_key(&destination(root, key)?));
            }
            for p in ["GTA5.exe", "update/update.rpf"] {
                claims.files.insert(super::super::path_key(&root.join(p)));
            }
        }
    }
    Ok(claims)
}
fn collision(paths: &BTreeSet<String>, p: &Path) -> bool {
    super::super::replacement::overlaps(paths, &super::super::path_key(p))
}
/// A generic uninstall rollback or trash restore may re-enable an older
/// install at completely disjoint destinations. Path checks alone cannot
/// protect the game/immutable-artifact slot in that case.
fn check_foreign_activations(e: &FileEngine, id: &str, claims: &Protection) -> AppResult<()> {
    if claims.artifacts.is_empty() {
        return Ok(());
    }
    let conn = e.db.conn();
    let mut st = conn.prepare("WITH target AS (SELECT id, status FROM file_transactions WHERE id=?1),
      activating(install_id) AS (
        SELECT a.install_id FROM target t JOIN install_transactions a ON a.transaction_id=t.id
          WHERE (t.status='committed' AND a.action='uninstall') OR (t.status<>'committed' AND a.action='install')
        UNION SELECT r.replaced_install_id FROM target t JOIN install_replacements r ON r.transaction_id=t.id WHERE t.status='committed'
        UNION SELECT a.install_id FROM target t JOIN trash_restore_links r ON r.transaction_id=t.id
          JOIN trash_items b ON b.id=r.item_id JOIN install_transactions a ON a.transaction_id=b.transaction_id
          WHERE t.status<>'committed' AND a.action='uninstall'
      ) SELECT i.game_id,json_extract(i.metadata_json,'$.libraryEntryId'),json_extract(i.metadata_json,'$.downloadOrigin.artifactId')
        FROM activating a JOIN installs i ON i.id=a.install_id")?;
    let rows = st.query_map([id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })?;
    for row in rows {
        let (game, library, download) = row?;
        if [library, download]
            .into_iter()
            .flatten()
            .any(|a| claims.artifacts.contains(&(game.clone(), a)))
        {
            return Err(conflict("不能重新激活已由 GTA RPF 托管的同一原包"));
        }
    }
    Ok(())
}
/// Invoked at apply AND rollback of other transaction kinds, not just at plan
/// creation. Moving a parent directory is also a write to the protected files.
pub(crate) fn guard_foreign_plan(e: &FileEngine, p: &FilePlan) -> AppResult<()> {
    let keys = protected(e, None)?;
    check_foreign_activations(e, &p.id, &keys)?;
    let mut private = vec![store(e)];
    if let Ok(dir) = jobs(e) {
        private.push(dir.with_file_name("gta-rpf-tools"));
        private.push(dir);
    }
    let mut reserved = Vec::new();
    for root in private {
        let exists = match std::fs::symlink_metadata(&root) {
            Ok(_) => true,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => return Err(err.into()),
        };
        reserved.push((super::super::path_key(&root), exists));
    }
    let touches_private = |p: &Path| {
        let key = super::super::path_key(p);
        reserved.iter().any(|(root, exists)| {
            key == *root
                || key
                    .strip_prefix(root)
                    .is_some_and(|rest| rest.starts_with('/'))
                || (*exists
                    && root
                        .strip_prefix(&key)
                        .is_some_and(|rest| rest.starts_with('/')))
        })
    };
    for entry in &p.entries {
        let op = &entry.operation;
        let dst = op.dst();
        if collision(&keys.files, dst)
            || touches_private(dst)
            || op.src().is_some_and(|src| {
                touches_private(src)
                    || matches!(op, FileOp::Move { .. }) && collision(&keys.files, src)
            })
        {
            return Err(conflict("通用事务涉及 GTA 受保护的共享文件或任务目录"));
        }
    }
    Ok(())
}
fn checked_job(e: &FileEngine, j: &Frozen, c: &CancellationToken) -> AppResult<State> {
    if j.input.inspect_dlc.is_some() {
        return Err(invalid(
            "DLC 探测任务不能安装；请用 inspectJobId 生成正式任务",
        ));
    }
    if binding(e, &j.binding.game, c)? != j.binding {
        return Err(conflict("RPF 任务的游戏或原版指纹已变化"));
    }
    let (head, state) = head(e, &j.binding.game)?;
    if head != j.parent {
        return Err(conflict(
            "已有其他 RPF 安装或卸载，请从当前贡献集合重新制作任务",
        ));
    }
    check_active(e, &j.binding, &state, c)?;
    verify_archives(e, &j.binding, &state.items)?;
    let external = protected(e, Some(&j.binding.game))?;
    let next = after_items(&state, &j.item, j.install)?;
    if j.install {
        ensure_inactive(e, &j.binding, &j.item)?;
        verify_archives(e, &j.binding, std::slice::from_ref(&j.item))?;
    } else if super::super::get_install(e, &j.item.id)?.status != "enabled" {
        return Err(conflict("RPF 卸载状态已变化"));
    }
    let mut refs = BTreeMap::new();
    for item in next.iter().chain(std::iter::once(&j.item)) {
        for b in item.contribution.refs() {
            refs.insert(&b.sha256, b);
        }
    }
    for v in j
        .bases
        .values()
        .flatten()
        .chain(j.input.targets.iter().filter_map(|t| t.baseline.as_ref()))
    {
        refs.insert(&v.sha256, v);
    }
    for b in refs.values() {
        if file_fp(&blob_path(e, b)?, c)? != Some(b.fp()) {
            return Err(conflict("持久 RPF 基线或贡献快照发生漂移"));
        }
    }
    for key in j.item.keys() {
        let dst = destination(&j.binding.root, &key)?;
        e.stable_path(&dst, false)?;
        if collision(&external.files, &dst) {
            return Err(conflict("另一个游戏的 RPF 贡献已占用同一路径"));
        }
        super::super::verify_unowned_paths(e, [dst.as_path()])?;
        let before = state
            .files
            .get(&key)
            .or_else(|| j.bases.get(&key).and_then(Option::as_ref));
        if file_fp(&dst, c)? != before.map(Blob::fp) {
            return Err(conflict("RPF 安装目标在准备后已改变"));
        }
    }
    Ok(state)
}
fn operation(
    e: &FileEngine,
    j: &Frozen,
    key: &str,
    new: Option<&Blob>,
    index: usize,
) -> AppResult<FileOp> {
    let dst = destination(&j.binding.root, key)?;
    if let Some(b) = new {
        Ok(FileOp::Copy {
            src: blob_path(e, b)?,
            dst,
        })
    } else {
        Ok(FileOp::Move {
            src: dst,
            dst: store(e)
                .join("retired")
                .join(&j.input.job_id)
                .join(index.to_string()),
        })
    }
}

fn deploy(
    e: &FileEngine,
    game: &str,
    identity: &str,
    job: &str,
    install: bool,
    organization: Option<OrganizationSuggestion>,
    c: &CancellationToken,
) -> AppResult<InstallPreview> {
    let guard = transaction::write_lock_cancellable(c)?;
    pending(e)?;
    let j = frozen(e, job)?;
    if j.binding.game != game
        || j.install != install
        || identity
            != if install {
                &j.item.artifact
            } else {
                &j.item.id
            }
    {
        return Err(conflict("RPF 任务与此游戏、原包、安装记录或操作不相符"));
    }
    let prior = checked_job(e, &j, c)?;
    let items = after_items(&prior, &j.item, install)?;
    require_workspace(e, &j)?;
    let mut outputs = accept(e, &j.input, c)?;
    require_workspace(e, &j)?;
    let mut state = State {
        binding: Some(j.binding.clone()),
        items,
        files: prior.files.clone(),
        bases: j.bases.clone(),
    };
    let mut ops = vec![];
    let mut expected = vec![];
    for (idx, key) in j.item.keys().into_iter().enumerate() {
        let alive = state.items.iter().any(|i| i.keys().contains(&key));
        let new = if alive {
            if key.starts_with("dlc:") {
                let Contribution::Dlc { source, .. } = &j.item.contribution else {
                    return Err(conflict("DLC 贡献无效"));
                };
                Some(source.clone())
            } else {
                Some(
                    outputs
                        .remove(&key)
                        .ok_or_else(|| invalid("遗漏 RPF 产物"))?,
                )
            }
        } else {
            state
                .bases
                .get(&key)
                .cloned()
                .ok_or_else(|| invalid("缺少恢复基线"))?
        };
        let before = prior
            .files
            .get(&key)
            .or_else(|| j.bases[&key].as_ref())
            .map(Blob::fp);
        let op = operation(e, &j, &key, new.as_ref(), idx)?;
        expected.push(if new.is_some() {
            (before, new.as_ref().map(Blob::fp))
        } else {
            (None, before)
        });
        ops.push(op);
        if alive {
            state.files.insert(key, new.unwrap());
        } else {
            state.files.remove(&key);
            state.bases.remove(&key);
        }
    }
    if !outputs.is_empty() {
        return Err(invalid("未声明的 RPF 产物"));
    }
    let artifact = mod_library::verify_for_install(e, game, &j.item.artifact)?;
    if artifact.entry.sha256 != j.item.digest {
        return Err(conflict("原包不再匹配 RPF 任务"));
    }
    let action = if install { "install" } else { "uninstall" };
    let summary = format!(
        "{} GTA RPF 贡献：{}",
        if install { "安装" } else { "卸载" },
        artifact.entry.name
    );
    let mut plan = e.build_locked_plan(&guard, ops, summary, true)?;
    plan.requires_review = true;
    if plan.entries.len() != expected.len()
        || plan
            .entries
            .iter()
            .zip(expected)
            .any(|(f, (d, s))| f.before != d || f.source != s)
    {
        return Err(conflict("RPF 目标或冻结源在计划期间变化"));
    }
    super::super::require_install_space(&plan, &j.binding.root, c)?;
    let mod_type = format!("rpf-{}", j.item.contribution.kind());
    let warn = vec!["此计划只验证归档与文件，不代表已在游戏内成功加载".to_owned()];
    let mut meta = json!({"gtaRpf":true,"adapterSlug":j.binding.slug,"adapterVersion":j.binding.version,"libraryEntryId":j.item.artifact,"artifactSha256":j.item.digest,"artifactPath":artifact.archive_path,"downloadOrigin":artifact.origin,"sourceModId":artifact.entry.project_id,"modType":mod_type,"transactionId":plan.id,"rpfJobId":job,"warnings":warn});
    if install {
        crate::mod_organization::stage(&e.db, &mut meta, organization);
    }
    check_cancel(c)?;
    let mut con = e.db.conn();
    let tx = con.transaction()?;
    FileEngine::persist_plan_kind(&tx, &plan, KIND)?;
    let now = util::now();
    if install {
        let name = &artifact.entry.name;
        tx.execute("INSERT INTO installs(id,game_id,name,version,source,status,install_root,files_json,metadata_json,installed_at,updated_at) VALUES(?1,?2,?3,?4,?5,'planned',?6,'[]',?7,?8,?8)",params![j.item.id,game,name,artifact.entry.version,artifact.entry.source_id,j.binding.root.to_string_lossy(),meta.to_string(),now])?;
    }
    tx.execute("INSERT INTO install_transactions(install_id,transaction_id,action,created_at) VALUES(?1,?2,?3,?4)",params![j.item.id,plan.id,action,now])?;
    tx.execute(
        "UPDATE file_transactions SET game_id=?1 WHERE id=?2",
        params![game, plan.id],
    )?;
    tx.execute("INSERT INTO gta_rpf_changes(transaction_id,job_id,game_id,state,plan_sha) VALUES(?1,?2,?3,?4,?5)",params![plan.id,job,game,serde_json::to_string(&state)?,plan_hash(&plan)?])?;
    tx.commit()?;
    drop(con);
    clean_consumed(e, &j);
    Ok(InstallPreview {
        game_id: game.into(),
        install_id: j.item.id,
        mod_type,
        install_root: j.binding.root,
        artifact_sha256: j.item.digest,
        download_origin: artifact.origin,
        plan,
        warnings: warn,
        ignored_files: vec![],
        requires_review: true,
    })
}
pub(crate) fn plan_install(
    e: &FileEngine,
    game_id: &str,
    artifact_id: &str,
    job_id: &str,
    organization: Option<OrganizationSuggestion>,
    cancel: &CancellationToken,
) -> AppResult<InstallPreview> {
    deploy(e, game_id, artifact_id, job_id, true, organization, cancel)
}
pub(crate) fn plan_uninstall(
    e: &FileEngine,
    game_id: &str,
    install_id: &str,
    job_id: &str,
    cancel: &CancellationToken,
) -> AppResult<FilePlan> {
    Ok(deploy(e, game_id, install_id, job_id, false, None, cancel)?.plan)
}
fn change(e: &FileEngine, plan: &FilePlan) -> AppResult<(Frozen, State)> {
    let (job,s,sha,game,id,action):(String,String,String,String,String,String)=e.db.conn().query_row("SELECT c.job_id,c.state,c.plan_sha,c.game_id,a.install_id,a.action FROM gta_rpf_changes c JOIN install_transactions a ON a.transaction_id=c.transaction_id JOIN file_transactions t ON t.id=c.transaction_id AND t.game_id=c.game_id AND t.kind='gta_rpf' WHERE c.transaction_id=?1",[&plan.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
    let j = frozen(e, &job)?;
    if plan_hash(plan)? != sha
        || !plan.requires_review
        || game != j.binding.game
        || id != j.item.id
        || action != if j.install { "install" } else { "uninstall" }
    {
        return Err(conflict("RPF 审批计划与冻结任务绑定不符"));
    }
    Ok((j, serde_json::from_str(&s)?))
}
pub(crate) fn validate_apply(e: &FileEngine, plan: &FilePlan) -> AppResult<()> {
    let c = CancellationToken::new();
    let (j, state) = change(e, plan)?;
    let before = checked_job(e, &j, &c)?;
    let keys = j.item.keys();
    if keys.len() != plan.entries.len() {
        return Err(conflict("RPF 计划操作数不匹配"));
    }
    for (idx, (key, entry)) in keys.iter().zip(&plan.entries).enumerate() {
        let b = state
            .files
            .get(key)
            .or_else(|| j.bases.get(key).and_then(Option::as_ref));
        let op = operation(e, &j, key, b, idx)?;
        let expected_before = before
            .files
            .get(key)
            .or_else(|| j.bases.get(key).and_then(Option::as_ref))
            .map(Blob::fp);
        if entry.operation != op
            || match b {
                Some(v) => {
                    entry.before != expected_before
                        || entry.source != Some(v.fp())
                        || file_fp(&blob_path(e, v)?, &c)? != Some(v.fp())
                }
                None => entry.before.is_some() || entry.source != expected_before,
            }
        {
            return Err(conflict("RPF 计划目标、来源或文件指纹与冻结数据不符"));
        }
    }
    Ok(())
}
pub(crate) fn validate_rollback(e: &FileEngine, id: &str) -> AppResult<()> {
    let c = CancellationToken::new();
    let plan = e.get_plan(id)?;
    let (j, state) = change(e, &plan)?;
    if head(e, &j.binding.game)?.0.as_deref() != Some(id) {
        return Err(conflict("只能回滚该游戏最新的 RPF 事务"));
    }
    if binding(e, &j.binding.game, &c)? != j.binding {
        return Err(conflict("游戏版本已变化，不能回滚"));
    }
    check_active(e, &j.binding, &state, &c)?;
    if !j.install {
        // Undoing an uninstall is a reactivation; a generic install of the
        // same artifact may have won the slot since then, at disjoint paths.
        ensure_inactive(e, &j.binding, &j.item)?;
        verify_archives(e, &j.binding, std::slice::from_ref(&j.item))?;
    }
    let other = protected(e, Some(&j.binding.game))?;
    for (k, ent) in j.item.keys().iter().zip(plan.entries) {
        let p = destination(&j.binding.root, k)?;
        if collision(&other.files, &p) {
            return Err(conflict("回滚目标与其他游戏共享"));
        }
        super::super::verify_unowned_paths(e, [p.as_path()])?;
        if file_fp(ent.operation.dst(), &c)? != ent.source
            || matches!(ent.operation, FileOp::Move { .. }) && file_fp(&p, &c)?.is_some()
        {
            return Err(conflict("RPF 回滚文件或保留副本已漂移"));
        }
    }
    Ok(())
}
/// A bounded verifier uses the current composite, not a stale per-install
/// hash. Inactive IDs have no manifest. This lookup does not touch game files.
pub(crate) fn active_files(
    e: &FileEngine,
    game: &str,
    id: &str,
) -> AppResult<Vec<super::super::InstalledFile>> {
    let (_, state) = head(e, game)?;
    let Some(item) = state.items.iter().find(|item| item.id == id) else {
        return Ok(vec![]);
    };
    let binding = state
        .binding
        .as_ref()
        .filter(|b| b.game == game)
        .ok_or_else(|| conflict("RPF 状态缺少游戏绑定"))?;
    item.keys()
        .into_iter()
        .map(|key| {
            let blob = state
                .files
                .get(&key)
                .ok_or_else(|| conflict("RPF 状态缺少目标指纹"))?;
            Ok(super::super::InstalledFile {
                path: destination(&binding.root, &key)?,
                sha256: blob.sha256.clone(),
                size: blob.size,
                before: state.bases.get(&key).and_then(Option::as_ref).map(Blob::fp),
            })
        })
        .collect()
}
/// Shared container fingerprints reflect the current generation, not the bytes
/// present when this individual contribution was added.
pub(crate) fn inspect(e: &FileEngine, installed: &InstalledMod) -> AppResult<InstallInspection> {
    let (_, s) = head(e, &installed.game_id)?;
    let item = s.items.iter().find(|v| v.id == installed.id);
    let c = CancellationToken::new();
    let mut files = vec![];
    let mut reasons = vec![];
    let mut blockers = BTreeSet::new();
    if let Some(i) = item {
        let b = s
            .binding
            .as_ref()
            .ok_or_else(|| conflict("RPF 状态缺少游戏绑定"))?;
        let root = &b.root;
        let live = binding(e, &installed.game_id, &c);
        match live {
            Ok(current) if current == *b => {}
            Ok(_) => reasons.push("游戏或适配包已变化".into()),
            Err(err) => reasons.push(err.to_string()),
        }
        let own = i.keys();
        // Preparing any uninstall verifies the complete live composite, not
        // only this install's destinations. Reflect that in reconciliation.
        for (k, blob) in &s.files {
            let p = destination(root, k)?;
            let fp = blob.fp();
            let current = e.stable_path(&p, false).and_then(|_| file_fp(&p, &c));
            let (actual, status) = match current {
                Ok(Some(v)) if v == fp => (Some(v), "owned"),
                Ok(Some(v)) => (Some(v), "modified"),
                Ok(None) => (None, "missing"),
                Err(_) => (None, "unreadable"),
            };
            let others: Vec<_> = s
                .items
                .iter()
                .filter(|x| x.id != i.id && x.keys().contains(k))
                .map(|x| x.id.clone())
                .collect();
            if status != "owned" {
                reasons.push(format!("{k}: {status}"));
                blockers.extend(others.iter().cloned());
            }
            if !own.contains(k) {
                continue;
            }
            files.push(super::super::InstallFileInspection {
                path: p,
                expected_fingerprint: fp,
                current_fingerprint: actual,
                state: status.into(),
                replaced: s.bases.get(k).is_some_and(Option::is_some),
                other_install_ids: others,
                reasons: vec![],
            });
        }
    } else {
        reasons.push("此 RPF 贡献不在当前已提交的活跃集合".into());
    }
    let can = installed.status == "enabled" && item.is_some() && reasons.is_empty();
    Ok(InstallInspection {
        install_id: installed.id.clone(),
        game_id: installed.game_id.clone(),
        status: installed.status.clone(),
        can_uninstall: can,
        files,
        reconciliation: super::super::InstallReconciliation {
            recommended_action: if can {
                "prepare_gta_rpf_job"
            } else {
                "inspect"
            }
            .into(),
            blocking_install_ids: blockers.into_iter().collect(),
            rollback_transaction_id: None,
            successor_install_id: None,
            reasons,
        },
    })
}
