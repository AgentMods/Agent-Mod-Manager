use super::*;
use crate::safe_fs::{DiskBudget, StagingDirectory};
const MAX_FILE: u64 = 4 * 1024 * 1024 * 1024;
const XML_LIMIT: u64 = 4 * 1024 * 1024;
pub(super) fn digest_ok(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn store(e: &FileEngine) -> PathBuf {
    e.paths.root.join("gta-rpf-private")
}
pub(super) fn jobs(e: &FileEngine) -> AppResult<PathBuf> {
    #[cfg(test)]
    {
        Ok(e.paths.root.join("test-gta-rpf-jobs"))
    }
    #[cfg(all(not(test), windows))]
    {
        let _ = e;
        let p = dirs::data_local_dir()
            .filter(|p| p.is_absolute())
            .ok_or_else(|| invalid("无法确定 Windows LocalAppData"))?;
        Ok(p.join("GlossMod/AMM/gta-rpf-jobs"))
    }
    #[cfg(all(not(test), not(windows)))]
    {
        let _ = e;
        Err(AppError::Unsupported("GTA RPF 仅支持 Windows".into()))
    }
}
pub(super) fn blob_path(e: &FileEngine, b: &Blob) -> AppResult<PathBuf> {
    if !digest_ok(&b.sha256, 64)
        || b.path != format!("inputs/{}", b.sha256)
        || b.size == 0
        || b.size > MAX_FILE
    {
        return Err(invalid("RPF 快照描述无效"));
    }
    Ok(store(e).join("blobs").join(&b.sha256))
}
/// No path-based copy of files controlled by the service: hold the source
/// handle, reject links/hardlinks, bound length and hash the bytes being copied.
fn consume(path: &Path, c: &CancellationToken, writer: impl Write, cap: u64) -> AppResult<Blob> {
    consume_bounded(path, c, writer, 1..=cap)
}

pub(super) fn consume_bounded(
    path: &Path,
    c: &CancellationToken,
    mut writer: impl Write,
    sizes: std::ops::RangeInclusive<u64>,
) -> AppResult<Blob> {
    let mut r = safe_fs::open_regular(path)?;
    safe_fs::verify_regular_at(&r, path)?;
    let before = r.metadata()?;
    let n = before.len();
    if !sizes.contains(&n) {
        return Err(invalid("RPF 文件大小超出允许范围"));
    }
    let mut hash = Sha256::new();
    let mut left = n;
    let mut block = [0u8; 65536];
    while left != 0 {
        check_cancel(c)?;
        let chunk = left.min(block.len() as u64) as usize;
        r.read_exact(&mut block[..chunk])?;
        writer.write_all(&block[..chunk])?;
        hash.update(&block[..chunk]);
        left -= chunk as u64;
    }
    let grew = r.read(&mut block[..1])? != 0;
    let after = r.metadata()?;
    if grew || after.len() != n || before.modified()? != after.modified()? {
        return Err(conflict("RPF 文件复制期间发生变化"));
    }
    safe_fs::verify_regular_at(&r, path)?;
    Ok(Blob::from_digest(hex::encode(hash.finalize()), n))
}
pub(super) fn file_fp(path: &Path, c: &CancellationToken) -> AppResult<Option<Fingerprint>> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => consume(path, c, std::io::sink(), MAX_FILE).map(|b| Some(b.fp())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub(super) fn freeze(
    e: &FileEngine,
    src: &Path,
    expected: Option<&str>,
    c: &CancellationToken,
) -> AppResult<Blob> {
    let parent = store(e).join("blobs");
    safe_fs::create_dir_all(&parent)?;
    let opened = safe_fs::open_regular(src)?;
    safe_fs::verify_regular_at(&opened, src)?;
    let size = opened.metadata()?.len();
    drop(opened);
    if let Some(hash) = expected {
        let b = Blob::from_digest(hash.into(), size);
        let p = blob_path(e, &b)?;
        if let Some(old) = file_fp(&p, c)? {
            if old == b.fp() && file_fp(src, c)? == Some(old) {
                return Ok(b);
            }
            return Err(conflict("RPF 原包或私有快照已变化"));
        }
    }
    DiskBudget::new(&parent)?.require(size)?;
    let staging =
        StagingDirectory::create(&parent.join(uuid::Uuid::new_v4().simple().to_string()))?;
    let _cleanup = staging.cleanup_guard()?;
    let tmp = staging.path().join("blob");
    let mut out = staging.create_file(Path::new("blob"))?;
    let b = consume(src, c, &mut out, MAX_FILE)?;
    out.sync_all()?;
    drop(out);
    if expected.is_some_and(|s| s != b.sha256) {
        return Err(conflict("冻结文件与预期 SHA-256 不符"));
    }
    let dst = blob_path(e, &b)?;
    match file_fp(&dst, c)? {
        None => safe_fs::rename_exclusive(&tmp, &dst)?,
        Some(old) if old == b.fp() => (),
        _ => return Err(conflict("RPF 基线或贡献快照损坏")),
    }
    Ok(b)
}
pub(super) fn check_shape(path: &Path, id: &str) -> AppResult<()> {
    let mut f = safe_fs::open_regular(path)?;
    safe_fs::verify_regular_at(&f, path)?;
    let size = f.metadata()?.len();
    if id == "menyoo" || id == "gameconfig" {
        if size > XML_LIMIT {
            return Err(invalid("RPF XML 超过大小限制"));
        }
        let mut s = String::new();
        f.take(XML_LIMIT + 1).read_to_string(&mut s)?;
        let doc = roxmltree::Document::parse(s.trim_start_matches('\u{feff}'))
            .map_err(|_| invalid("RPF XML 格式错误"))?;
        if doc.root_element().tag_name().name()
            != if id == "menyoo" {
                "AddedVehicleModels"
            } else {
                "fwAllConfigs"
            }
        {
            return Err(invalid("GTA XML 根元素无效"));
        }
    } else {
        let mut h = [0; 16];
        f.read_exact(&mut h)?;
        let count = u32::from_le_bytes(h[4..8].try_into().unwrap()) as u64;
        let names = u32::from_le_bytes(h[8..12].try_into().unwrap()) as u64;
        if h[..4] != *b"7FPR" || count == 0 || 16 + count * 16 + names > size {
            return Err(AppError::Presented(NativeMessage::new(
                "gta.rpf.invalid_rpf7",
                [],
            )));
        }
    }
    Ok(())
}

fn input_refs(input: &Input) -> AppResult<BTreeMap<String, Blob>> {
    let mut all = BTreeMap::new();
    if let Some(b) = &input.inspect_dlc {
        all.insert(b.path.clone(), b.clone());
    }
    for t in &input.targets {
        for b in t
            .baseline
            .iter()
            .chain(t.contributions.iter().flat_map(Contribution::refs))
        {
            if all
                .insert(b.path.clone(), b.clone())
                .as_ref()
                .is_some_and(|old| old != b)
            {
                return Err(conflict("RPF 任务内快照标识冲突"));
            }
        }
    }
    Ok(all)
}
pub(super) fn write_job(
    e: &FileEngine,
    input: &Input,
    c: &CancellationToken,
) -> AppResult<(safe_fs::StagingCleanup, safe_fs::FileIdentity)> {
    let parent = jobs(e)?;
    safe_fs::create_dir_all(&parent)?;
    let p = parent.join(&input.job_id);
    let dir = StagingDirectory::create(&p)?;
    let cleanup = dir.cleanup_guard()?;
    let refs = input_refs(input)?;
    let total = refs
        .values()
        .try_fold(0u64, |v, b| v.checked_add(b.size))
        .ok_or_else(|| invalid("RPF 任务过大"))?;
    if total > 32 * 1024 * 1024 * 1024 || refs.len() > 2048 {
        return Err(invalid("RPF 输入超过任务配额"));
    }
    DiskBudget::new(&p)?.require(total)?;
    dir.create_dir_all(Path::new("inputs"))?;
    for b in refs.values() {
        let path = blob_path(e, b)?;
        let mut out = dir.create_file(Path::new(&b.path))?;
        if consume(&path, c, &mut out, MAX_FILE)? != *b {
            return Err(conflict("RPF 私有输入指纹已变化"));
        }
        out.sync_all()?;
    }
    let data = serde_json::to_vec(input)?;
    if data.len() > 2 * 1024 * 1024 {
        return Err(invalid("RPF 任务清单过大"));
    }
    let mut file = dir.create_file(Path::new("input.json"))?;
    file.write_all(&data)?;
    file.sync_all()?;
    let identity = safe_fs::identity(&p)?.ok_or_else(|| conflict("RPF 任务目录身份不可用"))?;
    dir.verify_path()?;
    Ok((cleanup, identity))
}
pub(super) fn require_workspace(e: &FileEngine, j: &Frozen) -> AppResult<()> {
    if let Some(expected) = &j.workspace {
        if safe_fs::identity(&jobs(e)?.join(&j.input.job_id))?.as_ref() != Some(expected) {
            return Err(conflict("RPF 任务目录身份已改变"));
        }
    }
    Ok(())
}
/// Clean only jobs whose products (or derived job) have a durable DB binding.
/// Crashes, abandoned jobs and legacy rows without identity can leave orphans.
/// Journal blobs and retired targets remain for rollback; there is no blob GC.
fn retire_workspace(e: &FileEngine, j: &Frozen) -> AppResult<()> {
    let Some(expected) = &j.workspace else {
        return Ok(());
    };
    let parent = jobs(e)?;
    let src = parent.join(&j.input.job_id);
    if safe_fs::identity(&src)?.as_ref() != Some(expected) {
        return Err(conflict("RPF 任务目录已被替换，不做清理"));
    }
    // Rename the identified object into an exclusively created directory on
    // the same volume; never give the public job name to recursive deletion.
    let bin = StagingDirectory::create(
        &parent.join(format!(".consumed-{}", uuid::Uuid::new_v4().simple())),
    )?;
    let mut cleanup = bin.cleanup_guard()?;
    let retired = bin.path().join("job");
    safe_fs::rename_checked(&src, &retired, expected)?;
    if !matches!(safe_fs::identity(&retired), Ok(Some(ref id)) if id == expected) {
        cleanup.keep();
        return Err(conflict("RPF 任务目录清理身份不符，保留隔离副本"));
    }
    drop(bin);
    drop(cleanup);
    Ok(())
}
pub(super) fn clean_consumed(e: &FileEngine, j: &Frozen) {
    if let Err(err) = retire_workspace(e, j) {
        // Cleanup cannot turn a successful DB commit into a failed plan.
        tracing::warn!(job = %j.input.job_id, %err, "left consumed RPF workspace intact");
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ResultFile {
    schema_version: u8,
    job_id: String,
    status: String,
    outputs: Vec<Output>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    id: String,
    sha256: String,
    size: u64,
}
pub(super) fn small_file(path: &Path, max: u64) -> AppResult<Vec<u8>> {
    let mut v = Vec::new();
    consume(path, &CancellationToken::new(), &mut v, max)?;
    Ok(v)
}
pub(super) fn accept(
    e: &FileEngine,
    input: &Input,
    c: &CancellationToken,
) -> AppResult<BTreeMap<String, Blob>> {
    let job = jobs(e)?.join(&input.job_id);
    let expected: BTreeSet<_> = input.targets.iter().map(|t| t.id.as_str()).collect();
    let input_bytes = serde_json::to_vec(input)?;
    if small_file(&job.join("input.json"), 2 * 1024 * 1024)? != input_bytes {
        return Err(conflict("RPF 任务输入清单已变化"));
    }
    for b in input_refs(input)?.values() {
        if file_fp(&job.join(&b.path), c)? != Some(b.fp()) {
            return Err(conflict("交给 RPF 工具的输入已变化"));
        }
    }
    let result: ResultFile =
        serde_json::from_slice(&small_file(&job.join("result.json"), 64 * 1024)?)?;
    if result.schema_version != 1
        || result.status != "ok"
        || result.job_id != input.job_id
        || result.outputs.len() != expected.len()
    {
        return Err(invalid("RPF 结果与 native 任务不符"));
    }
    let out = job.join("output");
    let mut actual = BTreeSet::new();
    let identity = safe_fs::identity(&out)?.ok_or_else(|| invalid("RPF 结果缺少 output 目录"))?;
    for en in safe_fs::directory_entries(&out, &identity)? {
        let (n, dir) = en?;
        if dir || actual.len() >= 3 {
            return Err(invalid("RPF 暂存目录包含额外产物"));
        }
        actual.insert(n.into_string().map_err(|_| invalid("RPF 输出文件名无效"))?);
    }
    let expected_names = expected
        .iter()
        .map(|id| output_name(id).map(str::to_owned))
        .collect::<AppResult<BTreeSet<_>>>()?;
    if actual != expected_names {
        return Err(invalid("RPF 产物文件集合不匹配"));
    }
    let mut accepted = BTreeMap::new();
    for o in result.outputs {
        if !expected.contains(o.id.as_str())
            || accepted.contains_key(&o.id)
            || !digest_ok(&o.sha256, 64)
            || o.size == 0
            || o.size > MAX_FILE
        {
            return Err(invalid("RPF 产物重复、尺寸或摘要不合法"));
        }
        let b = freeze(e, &out.join(output_name(&o.id)?), Some(&o.sha256), c)?;
        if b.size != o.size {
            return Err(conflict("RPF 产物长度不符"));
        }
        check_shape(&blob_path(e, &b)?, &o.id)?;
        accepted.insert(o.id, b);
    }
    Ok(accepted)
}
