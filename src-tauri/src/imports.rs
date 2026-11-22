//! Desktop-selected, read-only external archives. Never expose this module as
//! an Agent tool: the caller must obtain an explicit native user selection.
#![cfg(not(any(target_os = "android", target_os = "ios")))]

use crate::{
    error::{AppError, AppResult},
    paths,
    transaction::{FileEngine, FileOp, FilePlan, Fingerprint},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

const MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const CHUNK_BYTES: usize = 64 * 1024;
static IMPORT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub filename: String,
    /// The original explicit selection, not an authorization root.
    #[serde(serialize_with = "crate::paths::serialize_user_path")]
    pub source_path: PathBuf,
    /// Destination of the reviewed COPY; it does not exist at preview time.
    #[serde(serialize_with = "crate::paths::serialize_user_path")]
    pub artifact_path: PathBuf,
    pub sha256: String,
    pub size: u64,
    #[serde(serialize_with = "crate::transaction::serialize_user_plan")]
    pub plan: FilePlan,
    pub warnings: Vec<String>,
}

fn denied(message: &str) -> AppError {
    AppError::Denied(message.into())
}
fn changed() -> AppError {
    AppError::Conflict(
        "Import source changed while reading; retained scratch is not approved".into(),
    )
}

fn filename(path: &Path) -> AppResult<String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| AppError::Invalid("Import filename must be valid UTF-8".into()))?;
    if name.len() > 180 || name.starts_with('.') || name.contains(['/', '\\']) {
        return Err(AppError::Invalid(
            "Import requires a single portable filename".into(),
        ));
    }
    // Match install's filename contract as well as the stricter archive device
    // name checks (including ADS and superscript Windows device numbers).
    crate::plugins::validate_relative_path(name, false)?;
    paths::safe_join(Path::new("filename-check"), name)?;
    let lower = name.to_ascii_lowercase();
    if ![".jar", ".zip", ".mrpack", ".7z", ".tar.gz", ".tgz"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
    {
        return Err(AppError::Unsupported(
            "Only jar/zip/mrpack/7z/tar.gz/tgz archives can be imported".into(),
        ));
    }
    Ok(name.into())
}

fn no_link(meta: &Metadata) -> AppResult<()> {
    if meta.file_type().is_symlink() {
        return Err(denied("Import paths cannot contain symbolic links"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(denied("Import paths cannot contain Windows reparse points"));
        }
    }
    Ok(())
}

/// Inspect before canonicalizing, otherwise a symlink would become invisible.
fn selected_path(engine: &FileEngine, path: &Path) -> AppResult<PathBuf> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(denied(
            "Import requires an absolute, non-traversing selected path",
        ));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        #[cfg(windows)]
        if let Component::Prefix(prefix) = component {
            if !matches!(
                prefix.kind(),
                std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
            ) {
                return Err(denied(
                    "Import requires a local disk file, not a device or network path",
                ));
            }
            current.push(component);
            continue; // A bare C: prefix is not the absolute C:\\ root yet.
        }
        current.push(component);
        let meta = fs::symlink_metadata(&current)?;
        no_link(&meta)?;
        if current != path && !meta.is_dir() {
            return Err(denied("Import path ancestors must be ordinary directories"));
        }
    }
    let resolved = path.canonicalize()?;
    // No manager-owned file is an external selection. This also explicitly
    // excludes renamed .zip metadata in secrets/logs/backups, not only manager.db.
    if resolved.starts_with(&engine.paths.root) {
        return Err(denied(
            "Manager database, secrets and internal files cannot be imported",
        ));
    }
    regular(&fs::symlink_metadata(&resolved)?)?;
    Ok(resolved)
}

fn regular(meta: &Metadata) -> AppResult<()> {
    no_link(meta)?;
    if !meta.is_file() {
        return Err(denied(
            "Import source must be an ordinary archive file, never a directory or special file",
        ));
    }
    if meta.len() > MAX_BYTES {
        return Err(AppError::Invalid(
            "Import archive exceeds the 4 GiB limit".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err(denied("Hard-linked archives are not accepted for import"));
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, u64, u32, i64, i64),
    #[cfg(windows)]
    identity: (u32, u32, u32),
}

fn stamp(file: &File) -> AppResult<Stamp> {
    let meta = file.metadata()?;
    regular(&meta)?;
    Ok(Stamp {
        size: meta.len(),
        modified: meta.modified()?,
        created: meta.created().ok(),
        #[cfg(unix)]
        identity: {
            use std::os::unix::fs::MetadataExt;
            (
                meta.dev(),
                meta.ino(),
                meta.nlink(),
                meta.mode(),
                meta.ctime(),
                meta.ctime_nsec(),
            )
        },
        #[cfg(windows)]
        identity: windows_identity(file)?,
    })
}

#[cfg(windows)]
fn windows_identity(file: &File) -> AppResult<(u32, u32, u32)> {
    use std::os::windows::io::AsRawHandle;
    // std's file_index/number_of_links accessors are unstable. Keep the small
    // stable Win32 identity query local rather than changing shared dependencies.
    #[repr(C)]
    #[derive(Default)]
    struct Information {
        attributes: u32,
        creation: [u32; 2],
        access: [u32; 2],
        write: [u32; 2],
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, info: *mut Information)
            -> i32;
    }
    let mut info = Information::default();
    // SAFETY: file owns the live handle and info is a correctly sized writable
    // BY_HANDLE_FILE_INFORMATION; the call does not retain either pointer.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if info.links != 1 || info.attributes & 0x400 != 0 {
        return Err(denied("Hard links and reparse points cannot be imported"));
    }
    Ok((info.volume, info.index_high, info.index_low))
}

fn open_source(path: &Path) -> AppResult<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Darwin O_NOFOLLOW_ANY | O_NONBLOCK. NOFOLLOW_ANY already includes
        // the leaf, and combining it with O_NOFOLLOW is EINVAL on Darwin.
        // Nonblocking avoids hanging on a FIFO substituted before open.
        options.custom_flags(0x2000_0000 | 0x4);
    }
    #[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "x86")))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x20000 | 0x800); // O_NOFOLLOW | O_NONBLOCK
    }
    #[cfg(all(target_os = "linux", any(target_arch = "aarch64", target_arch = "arm")))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x8000 | 0x800); // O_NOFOLLOW | O_NONBLOCK
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Open the reparse point itself; reject it before reading. Deny writers
        // and deletion/rename while the source handle is held.
        options.custom_flags(0x0020_0000).share_mode(0x1);
    }
    if !cfg!(any(
        target_os = "macos",
        windows,
        all(
            target_os = "linux",
            any(
                target_arch = "x86_64",
                target_arch = "x86",
                target_arch = "aarch64",
                target_arch = "arm"
            )
        )
    )) {
        return Err(AppError::Unsupported(
            "Safe native archive reads are unavailable on this platform".into(),
        ));
    }
    let file = options.open(path)?;
    stamp(&file)?;
    Ok(file)
}

fn unchanged(
    engine: &FileEngine,
    selected: &Path,
    canonical: &Path,
    file: &File,
    before: &Stamp,
) -> AppResult<()> {
    if selected_path(engine, selected)? != canonical || stamp(file)? != *before {
        return Err(changed());
    }
    // Check the named file as well as the held handle to catch replacement.
    if stamp(&open_source(canonical)?)? != *before {
        return Err(changed());
    }
    Ok(())
}

fn managed_path(engine: &FileEngine, path: &Path) -> AppResult<()> {
    engine.paths.stable_directory(&engine.paths.root)?;
    if !path.starts_with(engine.paths.staging()) {
        return Err(denied("Import output must remain in manager staging"));
    }
    let mut current = engine.paths.root.clone();
    for component in path
        .strip_prefix(&current)
        .map_err(|_| denied("Invalid staging path"))?
        .components()
    {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) => no_link(&meta)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(e.into()),
        }
    }
    if paths::ensure_within(path, &[engine.paths.staging()])? != path {
        return Err(denied("Import staging path was redirected"));
    }
    Ok(())
}

fn absent(path: &Path) -> AppResult<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
        Ok(_) => Err(AppError::Conflict(
            "Import never takes over an existing artifact or namespace".into(),
        )),
    }
}

#[derive(Default)]
struct Hooks {
    #[cfg(test)]
    callback: Option<Box<ReadHook>>,
}
#[cfg(test)]
type ReadHook = dyn FnMut(TestPoint, &Path, &Path, &Path) -> AppResult<()>;
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum TestPoint {
    Chunk,
    SnapshotReady,
    BeforePlan,
    BeforeEnginePlan,
}

/// The only external-read entry point. Native UI selection is a caller
/// precondition; no roots/Agent tools are registered here. Only new inert
/// scratch is written eagerly. Errors intentionally retain every staged byte.
pub fn plan_import_archive(engine: &FileEngine, path: &Path) -> AppResult<ImportPreview> {
    plan_import(engine, path, &mut Hooks::default())
}

fn plan_import(engine: &FileEngine, path: &Path, _hooks: &mut Hooks) -> AppResult<ImportPreview> {
    let _import = IMPORT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let filename = filename(path)?;
    let id = uuid::Uuid::new_v4().to_string();
    let scratch = engine
        .paths
        .staging()
        .join("import-scratch")
        .join(&id)
        .join(&filename);
    let artifact = engine
        .paths
        .staging()
        .join("imports")
        .join(&id)
        .join(&filename);
    let (size, sha256) = {
        // Release this lock before plan_file_ops/require_review, which each
        // acquire it themselves. Other native file workflows cannot mutate our
        // scratch while it is being materialized.
        let _files = crate::transaction::write_lock();
        let source = selected_path(engine, path)?;
        let mut input = open_source(&source)?;
        let before = stamp(&input)?;
        unchanged(engine, path, &source, &input, &before)?;
        managed_path(engine, &scratch)?;
        managed_path(engine, &artifact)?;
        absent(artifact.parent().unwrap())?;
        let directory = scratch.parent().unwrap();
        let parent = directory.parent().unwrap();
        fs::create_dir_all(parent)?;
        managed_path(engine, parent)?;
        fs::create_dir(directory)?; // Exclusive namespace; never reuse a UUID.
        managed_path(engine, &scratch)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = options.open(&scratch)?;
        let mut digest = Sha256::new();
        let mut size = 0;
        let mut buffer = [0u8; CHUNK_BYTES];
        // take() bounds even an unexpectedly growing file. Never write an
        // overflow byte to scratch; partial data remains on any failure.
        let mut bounded = (&mut input).take(before.size + 1);
        loop {
            let count = bounded.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            size += count as u64;
            if size > before.size || size > MAX_BYTES {
                return Err(changed());
            }
            output.write_all(&buffer[..count])?;
            digest.update(&buffer[..count]);
            #[cfg(test)]
            if let Some(hook) = _hooks.callback.as_mut() {
                hook(TestPoint::Chunk, path, &scratch, &artifact)?;
            }
        }
        output.sync_all()?;
        if size != before.size {
            return Err(changed());
        }
        unchanged(engine, path, &source, &input, &before)?;
        let sha256 = hex::encode(digest.finalize());
        #[cfg(test)]
        if let Some(hook) = _hooks.callback.as_mut() {
            hook(TestPoint::SnapshotReady, path, &scratch, &artifact)?;
        }
        // A second bounded pass proves that the snapshot matches a stable source,
        // rather than trusting timestamps alone (which can have coarse precision).
        input.seek(SeekFrom::Start(0))?;
        let mut digest = Sha256::new();
        let mut verified = 0;
        let mut bounded = (&mut input).take(before.size + 1);
        loop {
            let count = bounded.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            verified += count as u64;
            if verified > before.size {
                return Err(changed());
            }
            digest.update(&buffer[..count]);
        }
        unchanged(engine, path, &source, &input, &before)?;
        if verified != size || hex::encode(digest.finalize()) != sha256 {
            return Err(changed());
        }
        managed_path(engine, &scratch)?;
        (size, sha256)
    };
    #[cfg(test)]
    if let Some(hook) = _hooks.callback.as_mut() {
        hook(TestPoint::BeforePlan, path, &scratch, &artifact)?;
    }
    managed_path(engine, &scratch)?;
    managed_path(engine, &artifact)?;
    absent(artifact.parent().unwrap())?;
    #[cfg(test)]
    if let Some(hook) = _hooks.callback.as_mut() {
        hook(TestPoint::BeforeEnginePlan, path, &scratch, &artifact)?;
    }
    let plan = engine.plan_file_ops(
        vec![FileOp::Copy {
            src: scratch.clone(),
            dst: artifact.clone(),
        }],
        format!("Import local archive {filename}"),
    )?;
    let plan = engine.require_review(&plan.id)?;
    let expected = Fingerprint {
        kind: "file".into(),
        size,
        sha256: sha256.clone(),
    };
    let valid = plan.entries.len() == 1 && plan.entries.first().is_some_and(|entry| {
        matches!(&entry.operation, FileOp::Copy { src, dst } if src == &scratch && dst == &artifact)
            && entry.before.is_none()
            && entry.source.as_ref() == Some(&expected)
            && entry.impact == "add"
    });
    if !valid {
        // A preexisting target must never become an approvable overwrite, even
        // if it appeared between our absence check and the engine's planning lock.
        engine.db.conn().execute(
            "UPDATE file_transactions SET status='failed' WHERE id=?1 AND status='planned'",
            [&plan.id],
        )?;
        return Err(AppError::Conflict(
            "Import scratch or destination changed before planning".into(),
        ));
    }
    // Bind the desktop picker snapshot to its reviewed transaction. A generic
    // copy plan with similar paths does not produce an import receipt.
    engine.db.conn().execute(
        "INSERT INTO desktop_archive_imports(transaction_id, artifact_id, filename, size, sha256) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![plan.id, id, filename, size, sha256],
    )?;
    Ok(ImportPreview {
        filename,
        source_path: path.into(),
        artifact_path: artifact,
        sha256,
        size,
        plan,
        warnings: vec![
            "Only a read-only snapshot was staged. The destination changes only after native approval and apply; installation requires a separate reviewed plan.".into(),
            "SHA-256 proves snapshot integrity, not archive safety or compatibility. Nothing was extracted or executed. Failed and cancelled scratch files are retained.".into(),
        ],
    })
}

/// Only the applied, picker-originated snapshot can be published by this IPC.
/// Deriving the source from its UUID also survives storage relocation.
pub fn register_imported_mod(
    engine: &FileEngine,
    game_id: &str,
    transaction_id: &str,
) -> AppResult<crate::mod_library::LibraryEntry> {
    use rusqlite::OptionalExtension;
    let (artifact_id, name, size, hash): (String, String, u64, String) = engine
        .db
        .conn()
        .query_row(
            "SELECT i.artifact_id,i.filename,i.size,i.sha256 FROM desktop_archive_imports i
         JOIN file_transactions t ON t.id=i.transaction_id WHERE t.id=?1 AND t.status='committed'",
            [transaction_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?
        .ok_or_else(|| denied("仅能归类由桌面文件选择器导入且已提交的归档"))?;
    let game = crate::games::get_game(&engine.db, game_id)?;
    let archive = engine
        .paths
        .staging()
        .join("imports")
        .join(&artifact_id)
        .join(&name);
    crate::mod_library::ingest_import(
        engine,
        &crate::mod_library::ApprovedArtifact {
            artifact_id: &artifact_id,
            path: &archive,
            filename: &name,
            size,
            sha256: &hash,
        },
        &game.name,
        Some(game_id),
    )
}

#[cfg(test)]
#[path = "imports_tests.rs"]
mod tests;
