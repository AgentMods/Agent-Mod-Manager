//! Desktop Agent archive operations. Produce a new result in transaction-owned
//! staging, without letting an archive backend read a mutable directory tree.
use super::{
    has_path_overlap, ownership_conflict, require_identity, FileOp, Fingerprint, PlannedEntry,
};
use crate::{
    archive,
    error::{AppError, AppResult},
    paths, safe_fs,
    storage::AppPaths,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    ops::Bound::{Excluded, Unbounded},
    path::{Component, Path, PathBuf},
};

const MAX_ENTRIES: usize = 100_000;
const MAX_PATH_DEPTH: usize = 128;
const MAX_METADATA_BYTES: usize = 64 * 1024 * 1024;
const ENTRY_DISK_CHARGE: u64 = 4096;

struct Snapshot<'a> {
    target: &'a safe_fs::StagingDirectory,
    disk: safe_fs::DiskBudget<'a>,
    remaining: u64,
}

pub(super) fn validate_names(path: &Path) -> AppResult<()> {
    for part in path.components() {
        let Component::Normal(name) = part else {
            continue;
        };
        // Reject alternate spellings that Windows treats as the same filename
        // (trailing spaces/dots, ADS), also when examining a portable archive.
        let name = name.to_string_lossy();
        let name = std::ffi::OsStr::new(
            name.split(':')
                .next()
                .unwrap_or_default()
                .trim_end_matches([' ', '.']),
        );
        if AppPaths::env_name(name)
            || name.eq_ignore_ascii_case("private")
            || name.eq_ignore_ascii_case(".amm-retained")
        {
            return Err(AppError::Denied("归档操作不能访问受保护的路径".into()));
        }
    }
    Ok(())
}

fn validate_path_policy(paths: &AppPaths, root: &Path, relative: &Path) -> AppResult<()> {
    validate_names(relative)?;
    if (crate::paths::contains_path(&paths.downloads(), root)
        || crate::paths::contains_path(&paths.staging(), root))
        && relative.file_name().is_some_and(|name| {
            let name = name.to_string_lossy().to_ascii_lowercase();
            name.ends_with(".part") || name.ends_with(".rejected")
        })
    {
        return Err(AppError::Denied(
            "归档操作不能访问未完成或隔离的载荷".into(),
        ));
    }
    Ok(())
}

pub(super) fn require_new_destination(path: &Path) -> AppResult<()> {
    validate_names(path)?;
    match fs::symlink_metadata(path) {
        Ok(_) => Err(AppError::Conflict(format!(
            "归档操作的目标必须不存在: {}",
            paths::display_path(path)
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn comparison_key(path: &Path) -> PathBuf {
    if cfg!(any(windows, target_os = "macos")) {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    } else {
        path.into()
    }
}

pub(super) fn validate_plan_overlaps(entries: &[PlannedEntry]) -> AppResult<()> {
    if !entries.iter().any(|entry| entry.operation.is_archive()) {
        return Ok(());
    }
    let mut mutations = BTreeSet::new();
    let mut repeated = BTreeSet::new();
    for entry in entries {
        let op = &entry.operation;
        let moved = op.src().filter(|_| matches!(op, FileOp::Move { .. }));
        for path in std::iter::once(op.dst()).chain(moved) {
            let key = comparison_key(path);
            if !mutations.insert(key.clone()) {
                repeated.insert(key);
            }
        }
    }
    for entry in entries.iter().filter(|entry| entry.operation.is_archive()) {
        let dst = comparison_key(entry.operation.dst());
        let src = comparison_key(entry.operation.src().expect("archive source"));
        if has_path_overlap(&mutations, &src)
            || repeated.contains(&dst)
            || dst
                .ancestors()
                .skip(1)
                .any(|parent| mutations.contains(parent))
            || mutations
                .range::<Path, _>((Excluded(dst.as_path()), Unbounded))
                .next()
                .is_some_and(|p| p.starts_with(&dst))
        {
            return Err(AppError::Conflict(
                "归档的源或目标不能同时被本计划修改".into(),
            ));
        }
    }
    Ok(())
}

fn compression_format(dst: &Path) -> AppResult<&'static str> {
    match dst.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("zip") => Ok("zip"),
        Some(ext) if ext.eq_ignore_ascii_case("7z") => Ok("7z"),
        _ => Err(AppError::Invalid("压缩目标必须以 .zip 或 .7z 结尾".into())),
    }
}

/// Use held, non-following filesystem opens and reject multiply-linked files,
/// rather than allowing names within a tree to alias protected payloads.
fn file_fingerprint(
    path: &Path,
    mut copy: Option<(&mut Snapshot<'_>, File)>,
) -> AppResult<Fingerprint> {
    let mut input = safe_fs::open_regular(path)?;
    safe_fs::verify_regular_at(&input, path)?;
    let expected = input.metadata()?.len();
    if let Some((snapshot, _)) = &mut copy {
        if expected > snapshot.remaining {
            return Err(ownership_conflict(path));
        }
        snapshot.disk.require(expected)?;
    }
    let mut size = 0u64;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size += count as u64;
        if size > expected {
            return Err(ownership_conflict(path));
        }
        let bytes = &buffer[..count];
        digest.update(bytes);
        if let Some((snapshot, output)) = &mut copy {
            snapshot.disk.charge(count as u64)?;
            output.write_all(bytes)?;
        }
    }
    safe_fs::verify_regular_at(&input, path)?;
    if size != expected || input.metadata()?.len() != expected {
        return Err(ownership_conflict(path));
    }
    if let Some((snapshot, output)) = copy {
        output.sync_all()?;
        snapshot.remaining -= size;
    }
    Ok(Fingerprint {
        kind: "file".into(),
        size,
        sha256: hex::encode(digest.finalize()),
    })
}

fn portable_member(name: &std::ffi::OsStr) -> AppResult<&str> {
    let name = name
        .to_str()
        .ok_or_else(|| AppError::Invalid("归档条目名必须是 UTF-8".into()))?;
    if paths::safe_join(Path::new(""), name)?.as_os_str() != name {
        return Err(AppError::Denied("归档条目名不能包含路径分隔符".into()));
    }
    Ok(name)
}

fn directory_name(path: &Path) -> AppResult<&str> {
    portable_member(
        path.file_name()
            .ok_or_else(|| AppError::Invalid("压缩源必须是普通文件夹".into()))?,
    )
}

fn tree_fingerprint(
    paths: &AppPaths,
    tree: &Path,
    logical_root: &Path,
    mut copy: Option<&mut Snapshot<'_>>,
) -> AppResult<Fingerprint> {
    let identity = require_identity(tree)?;
    let mut todo = vec![(PathBuf::new(), identity.clone())];
    let mut entries = BTreeMap::new();
    let mut names = BTreeSet::new();
    let mut metadata_bytes = 0;
    let mut size = 0u64;
    while let Some((parent, id)) = todo.pop() {
        // join("") can append a trailing separator; the Windows safe_fs
        // primitives deliberately reject that non-canonical spelling.
        let dir = if parent.as_os_str().is_empty() {
            tree.to_path_buf()
        } else {
            tree.join(&parent)
        };
        for child in safe_fs::directory_entries(&dir, &id)? {
            let (name, is_dir) = child?;
            // A literal Unix backslash must not be reinterpreted as a separator
            // by the archive backend (nor encode an alternate Windows path).
            let rel = parent.join(portable_member(&name)?);
            let key = rel.to_string_lossy().to_lowercase();
            metadata_bytes += 128 + rel.as_os_str().len() + key.len();
            if entries.len() >= MAX_ENTRIES
                || metadata_bytes > MAX_METADATA_BYTES
                || rel.components().count() > MAX_PATH_DEPTH
            {
                return Err(AppError::Invalid(
                    "归档目录的条目数、路径深度或元数据超过限制".into(),
                ));
            }
            if !names.insert(key) {
                return Err(AppError::UnsafeArchive(format!(
                    "归档目录包含大小写冲突的路径: {}",
                    rel.display()
                )));
            }
            if let Some(snapshot) = copy.as_deref_mut() {
                snapshot.disk.charge(ENTRY_DISK_CHARGE)?;
            }
            validate_path_policy(paths, logical_root, &rel)?;
            let path = tree.join(&rel);
            let fp = if is_dir {
                todo.push((rel.clone(), require_identity(&path)?));
                if let Some(snapshot) = copy.as_deref_mut() {
                    snapshot.target.create_dir_all(&rel)?;
                }
                Fingerprint {
                    kind: "dir".into(),
                    size: 0,
                    sha256: String::new(),
                }
            } else {
                let file = match copy.as_deref_mut() {
                    Some(snapshot) => {
                        let file = snapshot.target.create_file(&rel)?;
                        Some((snapshot, file))
                    }
                    None => None,
                };
                file_fingerprint(&path, file)?
            };
            size = size
                .checked_add(fp.size)
                .ok_or_else(|| AppError::Invalid("归档树大小无法表示".into()))?;
            entries.insert(rel, fp);
        }
        if safe_fs::identity(&dir)?.as_ref() != Some(&id) {
            return Err(ownership_conflict(&dir));
        }
    }
    let mut digest = Sha256::new();
    for entry in entries {
        digest.update(serde_json::to_vec(&entry)?);
    }
    Ok(Fingerprint {
        kind: "dir".into(),
        size,
        sha256: hex::encode(digest.finalize()),
    })
}

pub(super) fn source_fingerprint(
    paths: &AppPaths,
    op: &FileOp,
    src: &Path,
    dst: &Path,
    check_format: bool,
) -> AppResult<Fingerprint> {
    validate_names(src)?;
    match op {
        FileOp::Compress { .. } => {
            compression_format(dst)?;
            directory_name(src)?;
            tree_fingerprint(paths, src, src, None)
        }
        FileOp::Extract { .. } => {
            let fp = file_fingerprint(src, None)?;
            if check_format {
                archive::detect_format(src)?;
            }
            Ok(fp)
        }
        _ => unreachable!("not an archive operation"),
    }
}

pub(super) fn output_fingerprint(
    paths: &AppPaths,
    op: &FileOp,
    staged: &Path,
) -> AppResult<Fingerprint> {
    validate_names(op.dst())?;
    match op {
        FileOp::Compress { .. } => file_fingerprint(staged, None),
        FileOp::Extract { dst, .. } => tree_fingerprint(paths, staged, dst, None),
        _ => unreachable!("not an archive operation"),
    }
}

pub(super) fn stage_output(
    paths: &AppPaths,
    op: &FileOp,
    source: &Option<Fingerprint>,
    work: &safe_fs::StagingDirectory,
) -> AppResult<(PathBuf, Fingerprint)> {
    let source = source
        .as_ref()
        .ok_or_else(|| AppError::Conflict("缺少归档源的预览指纹".into()))?;
    let output = match op {
        FileOp::Compress { src, dst } => {
            let format = compression_format(dst)?;
            let mut disk = safe_fs::DiskBudget::new(work.path())?;
            disk.charge(2 * ENTRY_DISK_CHARGE)?;
            disk.require(source.size)?;
            // Preserve the original root name as the archive's top-level directory.
            work.create_dir_all(Path::new("input"))?;
            let input = safe_fs::StagingDirectory::create(
                &work.path().join("input").join(directory_name(src)?),
            )?;
            let mut snapshot = Snapshot {
                target: &input,
                disk,
                remaining: source.size,
            };
            let copied = tree_fingerprint(paths, src, src, Some(&mut snapshot))?;
            if copied != *source {
                return Err(ownership_conflict(src));
            }
            input.verify_path()?;
            work.verify_path()?;
            let output = work.path().join(format!("payload.{format}"));
            archive::compress_snapshot(input.path(), &output, format)?;
            output
        }
        FileOp::Extract { src, .. } => {
            let output = work.path().join("payload");
            // The existing extractor validates the approved digest on the held
            // file and runs its decoders against a verified private snapshot.
            let mut result = archive::extract_verified_archive_cancellable(
                src,
                &output,
                &source.sha256,
                &tokio_util::sync::CancellationToken::new(),
            )?;
            result.cleanup.keep(); // The enclosing owned workspace now controls cleanup.
            output
        }
        _ => unreachable!("not an archive operation"),
    };
    work.verify_path()?;
    // Inspect the actual tree (including empty directories), not just the files
    // returned by the extractor. Apply policy to the projected user location.
    let fp = output_fingerprint(paths, op, &output)?;
    Ok((output, fp))
}
