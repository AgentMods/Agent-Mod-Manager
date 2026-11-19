use crate::{
    error::{AppError, AppResult},
    paths::safe_join,
    safe_fs::{self, StagingDirectory},
    util,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const MAX_ENTRY_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_COMPRESSION_RATIO: u64 = 100;
type WrittenFiles = Vec<(PathBuf, String)>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveEntry {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub compressed_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveListing {
    pub format: String,
    pub entries: Vec<ArchiveEntry>,
    pub total_size: u64,
    pub stripped_root: Option<String>,
}

fn unsafe_archive(message: impl Into<String>) -> AppError {
    AppError::UnsafeArchive(message.into())
}

fn is_root_directory(entry: &ArchiveEntry) -> bool {
    let name = entry.path.replace('\\', "/");
    entry.is_dir
        && !name.is_empty()
        && !name.starts_with('/')
        && name.split('/').all(|part| part.is_empty() || part == ".")
}

pub fn detect_format(path: &Path) -> AppResult<&'static str> {
    detect_format_from(path, &mut safe_fs::open_regular(path)?)
}

fn detect_format_from(path: &Path, file: &mut File) -> AppResult<&'static str> {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if [".zip", ".jar", ".mrpack", ".pak"]
        .iter()
        .any(|ext| name.ends_with(ext))
    {
        return Ok("zip");
    }
    if name.ends_with(".7z") {
        return Ok("7z");
    }
    if [".tar.gz", ".tgz", ".tar.bz2", ".tar.xz", ".tar"]
        .iter()
        .any(|ext| name.ends_with(ext))
    {
        return Ok("tar");
    }
    let mut buf = [0; 6];
    if file.read_exact(&mut buf).is_ok() {
        if &buf[..4] == b"PK\x03\x04" || &buf[..4] == b"PK\x05\x06" {
            return Ok("zip");
        }
        if &buf == b"7z\xbc\xaf\x27\x1c" {
            return Ok("7z");
        }
        if buf.starts_with(b"\x1f\x8b") || buf.starts_with(b"BZh") {
            return Ok("tar");
        }
    }
    Err(AppError::Unsupported(format!(
        "无法识别归档格式: {}",
        path.display()
    )))
}

fn check_entry(entry: &ArchiveEntry) -> AppResult<()> {
    // Validate the original name before stripping a root can hide traversal.
    if !is_root_directory(entry) {
        safe_join(Path::new(""), &entry.path)?;
    }
    if entry.size > MAX_ENTRY_BYTES {
        return Err(unsafe_archive(format!(
            "{} 超过单文件上限 4 GiB",
            entry.path
        )));
    }
    if entry.is_dir && entry.size != 0 {
        return Err(unsafe_archive(format!("目录包含数据: {}", entry.path)));
    }
    Ok(())
}

fn check_mode(mode: u32, is_dir: bool, name: &str) -> AppResult<()> {
    let kind = mode & 0o170000;
    if kind != 0 && kind != if is_dir { 0o040000 } else { 0o100000 } {
        return Err(unsafe_archive(format!(
            "链接、设备或特殊条目被拒绝: {name}"
        )));
    }
    Ok(())
}

fn check_declared(entries: &[ArchiveEntry], limit: u64) -> AppResult<()> {
    let mut total = 0u64;
    for entry in entries {
        check_entry(entry)?;
        total = total
            .checked_add(entry.size)
            .ok_or_else(|| unsafe_archive("归档大小溢出"))?;
        if total > limit {
            return Err(unsafe_archive("总解压比超过 100:1"));
        }
    }
    Ok(())
}

struct ReadBudget {
    total: u64,
    limit: u64,
}

/// Read at most one byte past a limit, and never write that byte to disk.
/// Require EOF at the declared size instead of trusting archive headers.
fn copy_bounded(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    name: &str,
    expected: u64,
    entry_limit: u64,
    budget: &mut ReadBudget,
) -> AppResult<()> {
    let mut actual = 0u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let remaining = entry_limit
            .saturating_sub(actual)
            .min(expected.saturating_sub(actual))
            .min(budget.limit.saturating_sub(budget.total));
        let len = remaining.saturating_add(1).min(buf.len() as u64) as usize;
        let n = match reader.read(&mut buf[..len]) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result.map_err(|e| unsafe_archive(format!("{name} 读取失败: {e}")))?,
        };
        if n == 0 {
            if actual != expected {
                return Err(unsafe_archive(format!("{name} 实际大小与声明不符")));
            }
            return Ok(());
        }
        if n as u64 > remaining {
            return Err(unsafe_archive(format!(
                "{name} 实际读取超过大小或总解压比限额"
            )));
        }
        actual += n as u64;
        budget.total += n as u64;
        writer.write_all(&buf[..n])?;
    }
}

struct Processing<'a> {
    dest: Option<&'a StagingDirectory>,
    strip_root: Option<&'a str>,
    entries: Vec<ArchiveEntry>,
    written: WrittenFiles,
    // Include implicit parents and case aliases for portable collision checks.
    names: HashMap<String, (PathBuf, bool)>,
    budget: ReadBudget,
}

impl<'a> Processing<'a> {
    fn new(
        dest: Option<&'a StagingDirectory>,
        strip_root: Option<&'a str>,
        limit: u64,
    ) -> AppResult<Self> {
        if let Some(root) = strip_root {
            let rel = safe_join(Path::new(""), root)?;
            if rel.components().count() != 1 || root.contains(['/', '\\']) || root == "." {
                return Err(unsafe_archive("无效的根目录前缀"));
            }
        }
        Ok(Self {
            dest,
            strip_root,
            entries: Vec::new(),
            written: Vec::new(),
            names: HashMap::new(),
            budget: ReadBudget { total: 0, limit },
        })
    }

    fn register(&mut self, path: &Path, is_dir: bool) -> AppResult<()> {
        let mut parts = PathBuf::new();
        let count = path.components().count();
        for (i, component) in path.components().enumerate() {
            parts.push(component.as_os_str());
            let directory = i + 1 < count || is_dir;
            let key = parts.to_string_lossy().to_lowercase();
            if let Some((previous, was_dir)) = self.names.get(&key) {
                if previous != &parts || !was_dir || !directory {
                    return Err(unsafe_archive(format!("条目路径冲突: {}", path.display())));
                }
            } else {
                self.names.insert(key, (parts.clone(), directory));
            }
        }
        Ok(())
    }

    fn entry(&mut self, entry: ArchiveEntry, reader: &mut dyn Read) -> AppResult<()> {
        check_entry(&entry)?;
        let relative = if is_root_directory(&entry) {
            String::new()
        } else {
            let normalized = safe_join(Path::new(""), &entry.path)?;
            strip_prefix(&normalized.to_string_lossy(), self.strip_root)
        };
        let relative = if relative.is_empty() {
            if !entry.is_dir {
                return Err(unsafe_archive("不能剥离文件条目"));
            }
            None
        } else {
            let rel = safe_join(Path::new(""), &relative)?;
            self.register(&rel, entry.is_dir)?;
            Some(rel)
        };
        let mut output = match (self.dest, relative.as_ref(), entry.is_dir) {
            (Some(dest), Some(relative), true) => {
                dest.create_dir_all(relative)?;
                None
            }
            (Some(dest), Some(relative), false) => Some((
                dest.path().join(relative),
                HashedOutput {
                    file: dest.create_file(relative)?,
                    hash: Sha256::new(),
                },
            )),
            _ => None,
        };
        let mut sink = std::io::sink();
        let writer: &mut dyn Write = match output.as_mut() {
            Some((_, file)) => file,
            None => &mut sink,
        };
        copy_bounded(
            reader,
            writer,
            &entry.path,
            entry.size,
            MAX_ENTRY_BYTES,
            &mut self.budget,
        )?;
        writer.flush()?;
        if let Some((path, file)) = output {
            self.written.push((path, hex::encode(file.hash.finalize())));
        }
        self.entries.push(entry);
        Ok(())
    }
}

struct HashedOutput {
    file: File,
    hash: Sha256,
}

impl Write for HashedOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.file.write(buf)?;
        self.hash.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn process_zip(file: &mut File, processing: &mut Processing<'_>) -> AppResult<()> {
    let mut zip = zip::ZipArchive::new(file)?;
    let mut entries = Vec::new();
    for i in 0..zip.len() {
        let entry = zip.by_index_raw(i)?;
        check_mode(entry.unix_mode().unwrap_or(0), entry.is_dir(), entry.name())?;
        entries.push(ArchiveEntry {
            path: entry.name().to_string(),
            is_dir: entry.is_dir(),
            size: entry.size(),
            compressed_size: entry.compressed_size(),
        });
    }
    check_declared(&entries, processing.budget.limit)?;
    for (i, entry) in entries.into_iter().enumerate() {
        processing.entry(entry, &mut zip.by_index(i)?)?;
    }
    Ok(())
}

fn process_7z(file: &mut File, length: u64, processing: &mut Processing<'_>) -> AppResult<()> {
    let mut reader = sevenz_rust::SevenZReader::new(file, length, sevenz_rust::Password::empty())
        .map_err(|e| unsafe_archive(format!("7z 读取失败: {e}")))?;
    let convert = |entry: &sevenz_rust::SevenZArchiveEntry| -> AppResult<ArchiveEntry> {
        if entry.is_anti_item
            || (entry.has_windows_attributes && entry.windows_attributes & (0x400 | 0x40) != 0)
        {
            return Err(unsafe_archive(format!(
                "7z 链接、设备或删除条目被拒绝: {}",
                entry.name
            )));
        }
        if entry.has_windows_attributes {
            check_mode(
                entry.windows_attributes >> 16,
                entry.is_directory(),
                &entry.name,
            )?;
        }
        Ok(ArchiveEntry {
            path: entry.name.clone(),
            is_dir: entry.is_directory(),
            size: entry.size,
            compressed_size: entry.compressed_size,
        })
    };
    let entries = reader
        .archive()
        .files
        .iter()
        .map(convert)
        .collect::<AppResult<Vec<_>>>()?;
    check_declared(&entries, processing.budget.limit)?;
    let mut unpacked = 0u64;
    for folder in &reader.archive().folders {
        if folder.total_output_streams > folder.unpack_sizes.len() {
            return Err(unsafe_archive("7z 解压流大小无效"));
        }
        unpacked = unpacked
            .checked_add(folder.get_unpack_size())
            .ok_or_else(|| unsafe_archive("7z 解压流大小溢出"))?;
    }
    let declared = entries
        .iter()
        .try_fold(0u64, |size, entry| size.checked_add(entry.size))
        .ok_or_else(|| unsafe_archive("7z 文件大小溢出"))?;
    if unpacked != declared {
        return Err(unsafe_archive("7z 解压流大小与文件大小不符"));
    }
    // High-level extraction creates a destination even with a custom callback.
    // The reader API has no filesystem side effects.
    reader
        .for_each_entries(|entry, data| {
            processing
                .entry(
                    convert(entry).map_err(|e| sevenz_rust::Error::other(e.to_string()))?,
                    data,
                )
                .map_err(|e| sevenz_rust::Error::other(e.to_string()))?;
            Ok(true)
        })
        .map_err(|e| unsafe_archive(format!("7z 读取失败: {e}")))
}

/// Bound the whole tar stream, including PAX metadata and trailing padding.
struct LimitedReader<R> {
    inner: R,
    remaining: u64,
}

impl<R: Read> Read for LimitedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let len = self.remaining.saturating_add(1).min(buf.len() as u64) as usize;
        let n = self.inner.read(&mut buf[..len])?;
        if n as u64 > self.remaining {
            return Err(std::io::Error::other("tar 实际解压比超过 100:1"));
        }
        self.remaining -= n as u64;
        Ok(n)
    }
}

fn process_tar(file: &mut File, processing: &mut Processing<'_>) -> AppResult<()> {
    let mut magic = [0u8; 6];
    let n = file.read(&mut magic)?;
    file.seek(SeekFrom::Start(0))?;
    let reader: Box<dyn Read + '_> = if magic[..n].starts_with(b"\x1f\x8b") {
        Box::new(flate2::read::MultiGzDecoder::new(file))
    } else if magic[..n].starts_with(b"BZh") {
        Box::new(bzip2::read::MultiBzDecoder::new(file))
    } else if magic[..n].starts_with(b"\xfd7zXZ\0") {
        return Err(AppError::Unsupported("暂不支持 tar.xz".into()));
    } else {
        Box::new(file)
    };
    let mut archive = tar::Archive::new(LimitedReader {
        inner: reader,
        remaining: processing.budget.limit,
    });
    for entry in archive
        .entries()
        .map_err(|e| unsafe_archive(e.to_string()))?
    {
        let mut entry = entry.map_err(|e| unsafe_archive(e.to_string()))?;
        let name = std::str::from_utf8(&entry.path_bytes())
            .map_err(|_| unsafe_archive("tar 条目名不是 UTF-8"))?
            .to_string();
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            return Err(unsafe_archive(format!(
                "tar 链接、设备或特殊条目被拒绝: {name}"
            )));
        }
        let info = ArchiveEntry {
            path: name,
            is_dir: kind.is_dir(),
            size: entry.size(),
            compressed_size: 0,
        };
        processing.entry(info, &mut entry)?;
    }
    std::io::copy(&mut archive.into_inner(), &mut std::io::sink())
        .map_err(|e| unsafe_archive(e.to_string()))?;
    Ok(())
}

fn process<'a>(
    file: &mut File,
    format: &str,
    dest: Option<&'a StagingDirectory>,
    strip_root: Option<&'a str>,
) -> AppResult<Processing<'a>> {
    file.rewind()?;
    let length = file.metadata()?.len();
    let mut processing = Processing::new(
        dest,
        strip_root,
        length.saturating_mul(MAX_COMPRESSION_RATIO),
    )?;
    match format {
        "zip" => process_zip(file, &mut processing)?,
        "7z" => process_7z(file, length, &mut processing)?,
        _ => process_tar(file, &mut processing)?,
    }
    Ok(processing)
}

pub fn list_zip(path: &Path) -> AppResult<Vec<ArchiveEntry>> {
    Ok(process(&mut safe_fs::open_regular(path)?, "zip", None, None)?.entries)
}

pub fn list_7z(path: &Path) -> AppResult<Vec<ArchiveEntry>> {
    Ok(process(&mut safe_fs::open_regular(path)?, "7z", None, None)?.entries)
}

pub fn list_tar_gz(path: &Path) -> AppResult<Vec<ArchiveEntry>> {
    Ok(process(&mut safe_fs::open_regular(path)?, "tar", None, None)?.entries)
}

pub fn list_archive(path: &Path) -> AppResult<ArchiveListing> {
    let mut file = safe_fs::open_regular(path)?;
    let format = detect_format_from(path, &mut file)?;
    build_listing(format, process(&mut file, format, None, None)?.entries)
}

pub(crate) fn list_verified_archive(path: &Path, expected: &str) -> AppResult<ArchiveListing> {
    let mut file = safe_fs::open_regular(path)?;
    verify_open_file(&mut file, expected)?;
    let format = detect_format_from(path, &mut file)?;
    let listing = build_listing(format, process(&mut file, format, None, None)?.entries)?;
    verify_open_file(&mut file, expected)?;
    Ok(listing)
}

fn build_listing(format: &str, entries: Vec<ArchiveEntry>) -> AppResult<ArchiveListing> {
    let total_size = entries
        .iter()
        .try_fold(0u64, |total, e| total.checked_add(e.size))
        .ok_or_else(|| unsafe_archive("归档大小溢出"))?;
    let names: Vec<String> = entries
        .iter()
        .map(|e| {
            if e.is_dir {
                format!("{}/", e.path.trim_end_matches(['/', '\\']))
            } else {
                e.path.clone()
            }
        })
        .collect();
    let stripped_root = crate::paths::strip_single_root(&names);
    Ok(ArchiveListing {
        format: format.to_string(),
        entries,
        total_size,
        stripped_root,
    })
}

/// Validate without writes, then extract exclusively into a newly created directory.
/// A write failure retains the partial staging directory for inspection/trash.
pub fn extract_archive(
    path: &Path,
    dest: &Path,
    strip_root: Option<&str>,
) -> AppResult<Vec<PathBuf>> {
    let mut file = safe_fs::open_regular(path)?;
    let format = detect_format_from(path, &mut file)?;
    Ok(extract_open_file(&mut file, format, dest, strip_root)?
        .1
        .into_iter()
        .map(|(path, _)| path)
        .collect())
}

fn extract_open_file(
    file: &mut File,
    format: &str,
    dest: &Path,
    strip_root: Option<&str>,
) -> AppResult<(ArchiveListing, WrittenFiles)> {
    let listing = build_listing(format, process(file, format, None, strip_root)?.entries)?;
    let staging = StagingDirectory::create(dest)?;
    let output = process(file, format, Some(&staging), strip_root)?;
    if listing.entries != output.entries {
        return Err(AppError::Conflict("归档内容在预检和解压之间已变化".into()));
    }
    staging.verify_path()?;
    Ok((listing, output.written))
}

pub(crate) struct VerifiedExtraction {
    pub(crate) listing: ArchiveListing,
    pub(crate) files: BTreeMap<PathBuf, String>,
}

pub(crate) fn extract_verified_archive(
    path: &Path,
    dest: &Path,
    expected_sha256: &str,
) -> AppResult<VerifiedExtraction> {
    let mut file = safe_fs::open_regular(path)?;
    verify_open_file(&mut file, expected_sha256)?;
    let format = detect_format_from(path, &mut file)?;
    let (listing, written) = extract_open_file(&mut file, format, dest, None)?;
    verify_open_file(&mut file, expected_sha256)?;
    Ok(VerifiedExtraction {
        listing,
        files: written.into_iter().collect(),
    })
}

fn verify_open_file(file: &mut File, expected: &str) -> AppResult<()> {
    file.rewind()?;
    if !util::sha256_reader(file)?.eq_ignore_ascii_case(expected) {
        return Err(AppError::Conflict(
            "载荷 SHA-256 在归档验证期间已变化".into(),
        ));
    }
    file.rewind()?;
    Ok(())
}

fn strip_prefix(name: &str, root: Option<&str>) -> String {
    let cleaned = name.replace('\\', "/");
    match root {
        Some(r) if cleaned.trim_end_matches('/') == r => String::new(),
        Some(r) => cleaned
            .strip_prefix(&format!("{r}/"))
            .unwrap_or(&cleaned)
            .to_string(),
        None => cleaned,
    }
}

#[cfg(test)]
#[path = "archive_path_tests.rs"]
mod path_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;

    fn fixture_dir() -> tempfile::TempDir {
        tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
    }

    fn zip_file(path: &Path, entries: &[(&str, &[u8])], deflate: bool) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default().compression_method(if deflate {
            zip::CompressionMethod::Deflated
        } else {
            zip::CompressionMethod::Stored
        });
        for (name, bytes) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn zip_traversal_is_rejected_in_preview_and_before_stripping() {
        let dir = fixture_dir();
        for (i, name) in [
            "../escape.txt",
            "/root/escape",
            "C:\\escape",
            "root/../escape",
            "root/file:ads",
            "root/CON.txt",
        ]
        .iter()
        .enumerate()
        {
            let path = dir.path().join(format!("{i}.zip"));
            zip_file(&path, &[(name, b"bad")], false);
            assert!(list_zip(&path).is_err(), "{name}");
            let dest = dir.path().join(format!("out{i}"));
            assert!(
                extract_archive(&path, &dest, Some("root")).is_err(),
                "{name}"
            );
            assert!(!dest.exists());
        }
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[test]
    fn zip_roundtrip_with_root_strip() {
        let dir = fixture_dir();
        let path = dir.path().join("mod.zip");
        zip_file(&path, &[("MyMod/data/thing.txt", b"hello")], false);
        let listing = list_archive(&path).unwrap();
        assert_eq!(listing.stripped_root.as_deref(), Some("MyMod"));
        let dest = dir.path().join("out");
        let written = extract_archive(&path, &dest, listing.stripped_root.as_deref()).unwrap();
        assert_eq!(written, [dest.join("data/thing.txt")]);
        assert_eq!(std::fs::read(&written[0]).unwrap(), b"hello");
    }

    #[test]
    fn existing_destination_and_duplicate_files_are_rejected() {
        let dir = fixture_dir();
        let path = dir.path().join("mod.zip");
        zip_file(&path, &[("data.txt", b"new")], false);
        let dest = dir.path().join("out");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join("data.txt"), b"old").unwrap();
        assert!(extract_archive(&path, &dest, None).is_err());
        assert_eq!(std::fs::read(dest.join("data.txt")).unwrap(), b"old");
        zip_file(&path, &[("data.txt", b"a"), ("DATA.txt", b"b")], false);
        assert!(list_archive(&path).is_err());
        assert!(extract_archive(&path, &dir.path().join("new"), None).is_err());
    }

    #[test]
    fn highly_compressed_zip_is_rejected() {
        let dir = fixture_dir();
        let path = dir.path().join("bomb.zip");
        zip_file(&path, &[("bomb", &vec![0; 1024 * 1024])], true);
        assert!(matches!(
            list_archive(&path),
            Err(AppError::UnsafeArchive(_))
        ));
        let dest = dir.path().join("out");
        assert!(extract_archive(&path, &dest, None).is_err());
        assert!(!dest.exists());
    }

    #[test]
    fn actual_reads_enforce_entry_and_total_limits_and_declared_size() {
        for (declared, entry_limit, total_limit, data) in [
            (9, 8, 100, b"123456789".as_slice()),
            (9, 100, 8, b"123456789".as_slice()),
            (8, 100, 100, b"123456789".as_slice()),
            (10, 100, 100, b"123456789".as_slice()),
        ] {
            let mut output = Vec::new();
            assert!(copy_bounded(
                &mut &*data,
                &mut output,
                "bomb",
                declared,
                entry_limit,
                &mut ReadBudget {
                    total: 0,
                    limit: total_limit
                }
            )
            .is_err());
            assert!(output.len() as u64 <= entry_limit.min(total_limit).min(declared));
        }
        let entry = ArchiveEntry {
            path: "huge".into(),
            is_dir: false,
            size: MAX_ENTRY_BYTES + 1,
            compressed_size: 1,
        };
        assert!(check_entry(&entry).is_err());
        let mut output = Vec::new();
        let mut budget = ReadBudget {
            total: 99,
            limit: 100,
        };
        assert!(copy_bounded(
            &mut &b"ab"[..],
            &mut output,
            "second",
            2,
            MAX_ENTRY_BYTES,
            &mut budget
        )
        .is_err());
        assert!(output.is_empty());
    }

    fn tar_file(
        path: &Path,
        name: &str,
        kind: tar::EntryType,
        declared: u64,
        data: &[u8],
        gzip: bool,
    ) {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(declared);
        header.set_entry_type(kind);
        // Construct unsafe names without Builder's path sanitization.
        header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
        header.set_cksum();
        let mut bytes = header.as_bytes().to_vec();
        bytes.extend_from_slice(data);
        bytes.resize(512 + data.len().div_ceil(512) * 512 + 1024, 0);
        if gzip {
            let mut encoder = flate2::write::GzEncoder::new(
                File::create(path).unwrap(),
                flate2::Compression::best(),
            );
            encoder.write_all(&bytes).unwrap();
            encoder.finish().unwrap();
        } else {
            std::fs::write(path, bytes).unwrap();
        }
    }

    #[test]
    fn tar_rejects_traversal_links_devices_fifo_and_oversize_in_preview() {
        let dir = fixture_dir();
        for (i, (name, kind, size)) in [
            ("../escape", tar::EntryType::Regular, 0),
            ("/absolute", tar::EntryType::Regular, 0),
            ("root/../escape", tar::EntryType::Directory, 0),
            ("link/", tar::EntryType::Symlink, 0),
            ("hard", tar::EntryType::Link, 0),
            ("char", tar::EntryType::Char, 0),
            ("block", tar::EntryType::Block, 0),
            ("fifo", tar::EntryType::Fifo, 0),
            ("huge", tar::EntryType::Regular, MAX_ENTRY_BYTES + 1),
        ]
        .into_iter()
        .enumerate()
        {
            let path = dir.path().join(format!("{i}.tar.gz"));
            tar_file(&path, name, kind, size, b"", true);
            assert!(list_archive(&path).is_err(), "{name}");
            let dest = dir.path().join(format!("out{i}"));
            assert!(
                extract_archive(&path, &dest, Some("root")).is_err(),
                "{name}"
            );
            assert!(!dest.exists());
        }
    }

    #[test]
    fn tar_and_gzip_roundtrip_and_read_limits() {
        let dir = fixture_dir();
        for gzip in [false, true] {
            let path = dir.path().join(if gzip { "mod.tar.gz" } else { "mod.tar" });
            tar_file(
                &path,
                "Mod/data.txt",
                tar::EntryType::Regular,
                5,
                b"hello",
                gzip,
            );
            let listing = list_archive(&path).unwrap();
            assert_eq!(listing.total_size, 5);
            let dest = dir.path().join(if gzip { "gzip" } else { "tar" });
            let written = extract_archive(&path, &dest, Some("Mod")).unwrap();
            assert_eq!(written, [dest.join("data.txt")]);
            assert_eq!(std::fs::read(&written[0]).unwrap(), b"hello");
        }
        let path = dir.path().join("bomb.tar.gz");
        tar_file(
            &path,
            "bomb",
            tar::EntryType::Regular,
            1024 * 1024,
            &vec![0; 1024 * 1024],
            true,
        );
        assert!(list_archive(&path).is_err());
        // tar iteration stops at the end marker; the gzip stream must still
        // be drained under its budget, including concatenated gzip members.
        tar_file(&path, "small", tar::EntryType::Regular, 0, b"", true);
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&vec![0; 1024 * 1024]).unwrap();
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&encoder.finish().unwrap())
            .unwrap();
        assert!(list_archive(&path).is_err());
    }

    fn sevenz_file(path: &Path, name: &str, attributes: Option<u32>, data: &[u8]) {
        let mut writer = sevenz_rust::SevenZWriter::create(path).unwrap();
        let mut entry = sevenz_rust::SevenZArchiveEntry::new();
        entry.name = name.into();
        entry.has_windows_attributes = attributes.is_some();
        entry.windows_attributes = attributes.unwrap_or(0);
        writer.push_archive_entry(entry, Some(data)).unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn sevenz_returns_written_files_without_placeholder_directories() {
        let dir = fixture_dir();
        let placeholders = [
            Path::new("__7z__").exists(),
            Path::new("__listing__").exists(),
        ];
        let path = dir.path().join("mod.7z");
        sevenz_file(&path, "Mod/data.txt", None, b"hello");
        let listing = list_archive(&path).unwrap();
        assert_eq!(listing.total_size, 5);
        let dest = dir.path().join("out");
        let written = extract_archive(&path, &dest, Some("Mod")).unwrap();
        assert_eq!(written, [dest.join("data.txt")]);
        assert_eq!(std::fs::read(&written[0]).unwrap(), b"hello");
        // All extraction directories must be inside the requested destination.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        assert_eq!(
            [
                Path::new("__7z__").exists(),
                Path::new("__listing__").exists()
            ],
            placeholders
        );
    }

    #[test]
    fn sevenz_rejects_traversal_unix_links_reparse_devices() {
        let dir = fixture_dir();
        for (i, (name, attrs)) in [
            ("../escape", None),
            ("/absolute", None),
            ("C:\\escape", None),
            ("a:stream", None),
            ("link", Some(0o120777 << 16)),
            ("char", Some(0o020666 << 16)),
            ("fifo", Some(0o010666 << 16)),
            ("reparse", Some(0x400)),
            ("device", Some(0x40)),
        ]
        .into_iter()
        .enumerate()
        {
            let path = dir.path().join(format!("{i}.7z"));
            sevenz_file(&path, name, attrs, b"bad");
            assert!(list_archive(&path).is_err(), "{name}");
            let dest = dir.path().join(format!("out{i}"));
            assert!(extract_archive(&path, &dest, None).is_err(), "{name}");
            assert!(!dest.exists());
        }
        let path = dir.path().join("bomb.7z");
        sevenz_file(&path, "bomb", None, &vec![0; 1024 * 1024]);
        assert!(list_archive(&path).is_err());
    }

    #[test]
    fn zip_rejects_directory_shaped_symlinks_and_special_modes() {
        let dir = fixture_dir();
        let path = dir.path().join("special.zip");
        for (mode, name) in [
            (0o120777u32, "link/"),
            (0o020666, "char"),
            (0o010666, "fifo"),
        ] {
            zip_file(&path, &[(name, b"")], false);
            let mut bytes = std::fs::read(&path).unwrap();
            let index = bytes.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
            bytes[index + 5] = 3; // Unix creator, with a forged external mode.
            bytes[index + 38..index + 42].copy_from_slice(&(mode << 16).to_le_bytes());
            std::fs::write(&path, bytes).unwrap();
            assert!(list_archive(&path).is_err(), "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn existing_and_dangling_destination_symlinks_are_rejected() {
        let dir = fixture_dir();
        let path = dir.path().join("mod.zip");
        zip_file(&path, &[("data", b"new")], false);
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let dest = dir.path().join("out");
        std::os::unix::fs::symlink(&outside, &dest).unwrap();
        assert!(extract_archive(&path, &dest, None).is_err());
        assert!(!outside.join("data").exists());
        std::fs::remove_file(&dest).unwrap();
        std::os::unix::fs::symlink(dir.path().join("missing"), &dest).unwrap();
        assert!(extract_archive(&path, &dest, None).is_err());
        assert!(!dir.path().join("missing").exists());
    }

    #[test]
    fn total_ratio_is_exact_and_allows_a_high_ratio_member_in_a_safe_archive() {
        let entry = ArchiveEntry {
            path: "data".into(),
            is_dir: false,
            size: 1000,
            compressed_size: 0,
        };
        assert!(check_declared(std::slice::from_ref(&entry), 1000).is_ok());
        let mut oversized = entry.clone();
        oversized.size = 1001;
        assert!(check_declared(&[oversized], 1000).is_err());
        let dir = fixture_dir();
        let path = dir.path().join("safe.zip");
        let mut state = 0x12345678u32;
        let noise: Vec<u8> = (0..32 * 1024)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        zip_file(
            &path,
            &[("high-ratio", &vec![0; 1024 * 1024]), ("noise", &noise)],
            true,
        );
        assert!(list_archive(&path).is_ok());
    }

    #[test]
    fn tar_dot_root_and_explicit_wrapper_directories_are_supported() {
        let dir = fixture_dir();
        let path = dir.path().join("mod.tar.gz");
        let encoder = flate2::write::GzEncoder::new(
            File::create(&path).unwrap(),
            flate2::Compression::default(),
        );
        let mut builder = tar::Builder::new(encoder);
        for (name, kind, data) in [
            ("./", tar::EntryType::Directory, b"".as_slice()),
            ("./Mod/", tar::EntryType::Directory, b"".as_slice()),
            ("./Mod/data", tar::EntryType::Regular, b"hello".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o755);
            header.set_size(data.len() as u64);
            header.set_entry_type(kind);
            builder.append_data(&mut header, name, data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        let listing = list_archive(&path).unwrap();
        assert_eq!(listing.stripped_root.as_deref(), Some("Mod"));
        let dest = dir.path().join("out");
        let written = extract_archive(&path, &dest, Some("Mod")).unwrap();
        assert_eq!(written, [dest.join("data")]);
    }

    #[test]
    fn tar_pax_traversal_and_truncated_data_are_rejected() {
        let dir = fixture_dir();
        let path = dir.path().join("pax.tar");
        let mut builder = tar::Builder::new(File::create(&path).unwrap());
        builder
            .append_pax_extensions([("path", b"../outside".as_slice())])
            .unwrap();
        let mut header = tar::Header::new_ustar();
        header.set_mode(0o644);
        header.set_size(1);
        header.set_entry_type(tar::EntryType::Regular);
        builder.append_data(&mut header, "safe", &b"x"[..]).unwrap();
        builder.finish().unwrap();
        assert!(list_archive(&path).is_err());
        let short = dir.path().join("short.tar");
        tar_file(&short, "short", tar::EntryType::Regular, 4096, b"x", false);
        assert!(list_archive(&short).is_err());
    }

    #[test]
    fn sevenz_anti_items_are_rejected_from_archive_metadata() {
        // Build an unencoded 7z header: FilesInfo, one empty file, Anti=1.
        // The dependency's writer does not faithfully emit anti-item flags.
        fn crc32(data: &[u8]) -> u32 {
            let mut crc = !0u32;
            for byte in data {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
                }
            }
            !crc
        }
        let header = [
            0x01, 0x05, 0x01, 0x0e, 0x01, 0x80, 0x0f, 0x01, 0x80, 0x10, 0x01, 0x80, 0x11, 0x05, 0,
            b'a', 0, 0, 0, 0, 0,
        ];
        let mut start = 0u64.to_le_bytes().to_vec();
        start.extend_from_slice(&(header.len() as u64).to_le_bytes());
        start.extend_from_slice(&crc32(&header).to_le_bytes());
        let mut data = b"7z\xbc\xaf\x27\x1c\0\x04".to_vec();
        data.extend_from_slice(&crc32(&start).to_le_bytes());
        data.extend_from_slice(&start);
        data.extend_from_slice(&header);
        let dir = fixture_dir();
        let path = dir.path().join("anti.7z");
        std::fs::write(&path, data).unwrap();
        let reader =
            sevenz_rust::SevenZReader::open(&path, sevenz_rust::Password::empty()).unwrap();
        assert!(reader.archive().files[0].is_anti_item);
        assert!(matches!(
            list_archive(&path),
            Err(AppError::UnsafeArchive(_))
        ));
    }
}
