use crate::{
    error::{AppError, AppResult},
    paths::{self, safe_join},
    safe_fs::{self, DiskBudget, FileIdentity, StagingDirectory},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

const MAX_ENTRIES: usize = 100_000;
const MAX_METADATA_BYTES: u64 = 64 * 1024 * 1024;
const ARCHIVE_HEADROOM: u64 = 64 * 1024;

fn absolute_lexical(path: &Path) -> AppResult<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(paths::lexical_normalize(&absolute))
}

fn portable_name(path: &Path, what: &str) -> AppResult<String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::Invalid(format!("{what} 必须包含有效的 UTF-8 文件名")))?;
    if safe_join(Path::new(""), name)?.components().count() != 1 {
        return Err(AppError::Invalid(format!("{what} 必须是普通路径名")));
    }
    Ok(name.to_owned())
}

fn directory_identity(path: &Path) -> AppResult<FileIdentity> {
    let identity = safe_fs::identity(path)?
        .ok_or_else(|| AppError::Denied(format!("目录不存在或不是普通目录: {}", path.display())))?;
    // Verify the snapshot root without following a replaced link or ancestor.
    drop(safe_fs::directory_entries(path, &identity)?);
    Ok(identity)
}

/// Estimate an upper bound for the archive output without copying the private
/// transaction snapshot a second time. Transaction code owns snapshot creation,
/// approval and the source tree fingerprint; this walk only checks its resource
/// envelope and refuses an unexpected link before handing it to 7-Zip.
fn archive_reserve(source: &Path, root_name: &str) -> AppResult<u64> {
    let mut entries = 1usize;
    let mut metadata_bytes = root_name.len() as u64 * 3 + 192;
    let mut file_bytes = 0u64;
    super::check_entry(&super::ArchiveEntry {
        path: root_name.to_owned(),
        is_dir: true,
        size: 0,
        compressed_size: 0,
    })?;

    for entry in walkdir::WalkDir::new(source)
        .follow_links(false)
        .min_depth(1)
    {
        let entry = entry?;
        entries = entries
            .checked_add(1)
            .filter(|count| *count <= MAX_ENTRIES)
            .ok_or_else(|| AppError::Invalid("目录条目数量超过压缩预算".into()))?;
        let kind = entry.file_type();
        if kind.is_symlink() || (!kind.is_file() && !kind.is_dir()) {
            return Err(AppError::Denied("事务快照包含链接或特殊文件".into()));
        }
        let relative = entry
            .path()
            .strip_prefix(source)
            .map_err(|error| AppError::Invalid(error.to_string()))?;
        let relative = relative
            .to_str()
            .ok_or_else(|| AppError::Invalid("事务快照包含非 UTF-8 路径名".into()))?
            .replace('\\', "/");
        let member = format!("{root_name}/{relative}");
        let size = if kind.is_file() {
            fs::symlink_metadata(entry.path())?.len()
        } else {
            0
        };
        super::check_entry(&super::ArchiveEntry {
            path: member.clone(),
            is_dir: kind.is_dir(),
            size,
            compressed_size: 0,
        })?;
        file_bytes = file_bytes
            .checked_add(size)
            .ok_or_else(|| AppError::Invalid("目录总大小溢出".into()))?;
        metadata_bytes = metadata_bytes
            .checked_add(192 + member.len() as u64 * 3)
            .filter(|bytes| *bytes <= MAX_METADATA_BYTES)
            .ok_or_else(|| AppError::Invalid("目录条目元数据超过压缩预算".into()))?;
    }

    file_bytes
        .checked_add(file_bytes / 50)
        .and_then(|bytes| bytes.checked_add(metadata_bytes))
        .and_then(|bytes| bytes.checked_add(ARCHIVE_HEADROOM))
        .ok_or_else(|| AppError::Invalid("压缩磁盘预算溢出".into()))
}

/// Compress a transaction-owned, private, approved snapshot directly with the
/// bundled 7-Zip executable. `src` must be the snapshot directory whose basename
/// is the original directory name; do not pass the mutable user source here.
/// The archive is atomically published without replacing an existing object.
pub(crate) fn compress_snapshot(src: &Path, output: &Path, format: &str) -> AppResult<()> {
    if !matches!(format, "zip" | "7z") {
        return Err(AppError::Invalid("压缩格式只能是 zip 或 7z".into()));
    }

    let source = absolute_lexical(src)?;
    let destination = absolute_lexical(output)?;
    let source_name = portable_name(&source, "压缩快照目录")?;
    portable_name(&destination, "压缩输出")?;
    let output_parent = destination
        .parent()
        .ok_or_else(|| AppError::Invalid("压缩输出必须有父目录".into()))?;
    if paths::overlaps(&source, &destination) {
        return Err(AppError::Denied("压缩快照与输出路径不能重叠".into()));
    }

    let source_identity = directory_identity(&source)?;
    directory_identity(output_parent)?;
    match fs::symlink_metadata(&destination) {
        Ok(_) => return Err(AppError::Conflict("压缩输出对象已存在".into())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if safe_fs::identity(&source)?.as_ref() != Some(&source_identity) {
        return Err(AppError::Conflict("压缩快照目录已被替换".into()));
    }

    let reserve = archive_reserve(&source, &source_name)?;
    DiskBudget::new(output_parent)?.require(reserve)?;

    let staging_path = output_parent.join(format!(".amm-compress-{}", uuid::Uuid::new_v4()));
    let cleanup;
    {
        let staging = StagingDirectory::create(&staging_path)?;
        cleanup = staging.cleanup_guard()?;
        let archive_name = if format == "zip" {
            "payload.zip"
        } else {
            "payload.7z"
        };
        let archive_path = staging.path().join(archive_name);
        let source_argument = Path::new(".").join(&source_name);
        let mut command = crate::seven_zip::command()?;
        let status = command
            .current_dir(source.parent().expect("snapshot path has a parent"))
            .args([
                "a",
                if format == "zip" { "-tzip" } else { "-t7z" },
                "-mx=1",
                "-bd",
                "-y",
                "-r",
                "--",
            ])
            .arg(&archive_path)
            .arg(source_argument)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(AppError::Other(format!("7-Zip 压缩失败: {status}")));
        }

        let archive_file = safe_fs::open_regular(&archive_path)?;
        safe_fs::verify_regular_at(&archive_file, &archive_path)?;
        if archive_file.metadata()?.len() == 0 {
            return Err(AppError::UnsafeArchive("7-Zip 生成了空归档".into()));
        }
        drop(archive_file);
        staging.verify_path()?;
        safe_fs::rename_exclusive(&archive_path, &destination)?;
    }
    drop(cleanup);
    Ok(())
}
