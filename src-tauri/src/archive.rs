use crate::{
    error::{AppError, AppResult},
    paths::safe_join,
    safe_fs::{self, StagingDirectory},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

#[path = "archive_compress.rs"]
mod archive_compress;
pub(crate) use archive_compress::compress_snapshot;
#[path = "archive_extract.rs"]
mod archive_extract;
#[path = "archive_rar.rs"]
mod archive_rar;
#[path = "archive_zip.rs"]
mod archive_zip;
#[cfg(test)]
#[path = "archive_rar_tests.rs"]
mod rar_tests;

// Metadata consumes memory even for empty files; charge names and parents
// separately from the byte limit for streamed payloads.
const METADATA_BYTES: usize = 64 * 1024 * 1024;
const SEVENZ_HEADER_KIB: usize = 4 * 1024;
const SEVENZ_HEADER_INPUT_BYTES: u64 = 16 * 1024 * 1024;
// The third-party 7z header decoder is opaque until it returns the block
// inventory. Retain a separate time guard here, not on streaming file data.
const SEVENZ_HEADER_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const SEVENZ_DICTIONARY_KIB: usize = 128 * 1024;
const MAX_TAR_EXTENSION: u64 = 64 * 1024;
const MAX_TAR_PADDING: u64 = 16 * 1024 * 1024;
const TAR_FRAMING_BYTES: u64 = 128 * 1024 * 1024;
static ARCHIVE_WORK: Mutex<()> = Mutex::new(());

struct Work<'a> {
    cancel: &'a CancellationToken,
}
impl<'a> Work<'a> {
    fn new(cancel: &'a CancellationToken) -> Self {
        Self { cancel }
    }
    fn check(&self) -> AppResult<()> {
        if self.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        Ok(())
    }
}

/// A bounded, cancellable view of one physical archive. Real progress is
/// measured in bytes, not by how quickly the filesystem or decoder runs.
/// ZIP preflight and 7z metadata can seek and read an extent more than once;
/// account for those reads too, so corrupt metadata cannot cause unbounded replay.
struct CheckedInput<'a, R> {
    inner: R,
    work: &'a Work<'a>,
    length: u64,
    position: u64,
    remaining_io: u64,
}
impl<'a, R> CheckedInput<'a, R> {
    fn new(inner: R, work: &'a Work<'a>, length: u64) -> Self {
        Self {
            inner,
            work,
            length,
            position: 0,
            remaining_io: length.saturating_mul(4).saturating_add(128 * 1024),
        }
    }
}
impl<R: Read> Read for CheckedInput<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.work.check().map_err(std::io::Error::other)?;
        let remaining = self.length - self.position;
        if remaining == 0 || buf.is_empty() {
            return Ok(0);
        }
        if self.remaining_io == 0 {
            return Err(std::io::Error::other("归档压缩数据重复读取超过字节预算"));
        }
        let len = remaining.min(self.remaining_io).min(buf.len() as u64) as usize;
        let n = self.inner.read(&mut buf[..len])?;
        self.position += n as u64;
        self.remaining_io -= n as u64;
        Ok(n)
    }
}
impl<R: Seek> Seek for CheckedInput<'_, R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.work.check().map_err(std::io::Error::other)?;
        let target = match pos {
            SeekFrom::Start(target) => Some(target),
            SeekFrom::End(delta) => self.length.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
        }
        .filter(|target| *target <= self.length)
        .ok_or_else(|| std::io::Error::other("归档数据偏移超出文件范围"))?;
        self.position = self.inner.seek(SeekFrom::Start(target))?;
        Ok(self.position)
    }
}

struct SevenZHeaderInput<R> {
    inner: R,
    started: Instant,
    remaining: u64,
}
impl<R> SevenZHeaderInput<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            started: Instant::now(),
            remaining: SEVENZ_HEADER_INPUT_BYTES,
        }
    }
    fn check(&self) -> std::io::Result<()> {
        if self.started.elapsed() > SEVENZ_HEADER_TIMEOUT {
            return Err(std::io::Error::other("7z 归档头解析超时"));
        }
        Ok(())
    }
}
impl<R: Read> Read for SevenZHeaderInput<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.check()?;
        if self.remaining == 0 && !buf.is_empty() {
            return Err(std::io::Error::other("7z 归档头超过压缩元数据读取预算"));
        }
        let len = self.remaining.min(buf.len() as u64) as usize;
        let n = self.inner.read(&mut buf[..len])?;
        self.remaining -= n as u64;
        Ok(n)
    }
}
impl<R: Seek> Seek for SevenZHeaderInput<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.check()?;
        self.inner.seek(pos)
    }
}
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

pub(crate) const RAR4_MAGIC: &[u8] = b"Rar!\x1a\x07\x00";
pub(crate) const RAR5_MAGIC: &[u8] = b"Rar!\x1a\x07\x01\x00";

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
    file.rewind()?;
    let mut buf = [0; 8];
    let length = file.metadata()?.len().min(buf.len() as u64) as usize;
    file.read_exact(&mut buf[..length])?;
    let buf = &buf[..length];
    if buf.starts_with(RAR4_MAGIC) || buf.starts_with(RAR5_MAGIC) {
        return Ok("rar");
    }
    if name.ends_with(".rar") {
        return Err(unsafe_archive("RAR 文件头无效、已截断或不受支持"));
    }
    if length >= 6 {
        if &buf[..4] == b"PK\x03\x04" || &buf[..4] == b"PK\x05\x06" {
            return Ok("zip");
        }
        if buf.starts_with(b"7z\xbc\xaf\x27\x1c") {
            return Ok("7z");
        }
        if buf.starts_with(b"\x1f\x8b") || buf.starts_with(b"BZh") {
            return Ok("tar");
        }
    }
    Err(AppError::Unsupported(format!(
        "无法识别归档格式: {}",
        crate::paths::display_path(path)
    )))
}

fn check_entry(entry: &ArchiveEntry) -> AppResult<()> {
    if entry.path.len() > 4096 || entry.path.split(['/', '\\']).count() > 128 {
        return Err(unsafe_archive("归档条目路径过长或层级过深"));
    }
    // Validate the original name before stripping a root can hide traversal.
    if !is_root_directory(entry) {
        safe_join(Path::new(""), &entry.path)?;
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
            return Err(unsafe_archive("声明解压大小超过可用磁盘预算"));
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
    budget: &mut ReadBudget,
    work: &Work<'_>,
) -> AppResult<()> {
    let mut actual = 0u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        work.check()?;
        let remaining = expected
            .saturating_sub(actual)
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
                "{name} 实际读取超过声明大小或可用磁盘预算"
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
    capture: Option<&'a BTreeMap<PathBuf, usize>>,
    captured: BTreeMap<PathBuf, ArchiveMemberPrefix>,
    // Include implicit parents and case aliases for portable collision checks.
    names: HashMap<String, (PathBuf, bool)>,
    budget: ReadBudget,
    disk: Option<safe_fs::DiskBudget<'a>>,
    metadata_left: usize,
    work: &'a Work<'a>,
}

impl<'a> Processing<'a> {
    fn new(
        dest: Option<&'a StagingDirectory>,
        strip_root: Option<&'a str>,
        limit: u64,
        work: &'a Work<'a>,
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
            capture: None,
            captured: BTreeMap::new(),
            names: HashMap::new(),
            budget: ReadBudget { total: 0, limit },
            disk: dest
                .map(|d| safe_fs::DiskBudget::new(d.path()))
                .transpose()?,
            metadata_left: METADATA_BYTES,
            work,
        })
    }

    fn metadata(&mut self, bytes: usize) -> AppResult<()> {
        self.metadata_left = self
            .metadata_left
            .checked_sub(bytes)
            .ok_or_else(|| unsafe_archive("归档路径与索引超过可用元数据预算"))?;
        Ok(())
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
                self.metadata(128 + key.len() + parts.as_os_str().len())?;
                if let Some(disk) = &mut self.disk {
                    disk.charge(4096)?;
                }
                self.names.insert(key, (parts.clone(), directory));
            }
        }
        Ok(())
    }

    fn entry(&mut self, entry: ArchiveEntry, reader: &mut dyn Read) -> AppResult<()> {
        self.work.check()?;
        check_entry(&entry)?;
        self.metadata(192 + entry.path.len() * 3)?;
        if entry.size > self.budget.limit.saturating_sub(self.budget.total) {
            return Err(unsafe_archive("声明解压大小超过可用磁盘预算"));
        }
        if let Some(disk) = &self.disk {
            disk.require(entry.size)?;
        }
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
        if let (Some(capture), Some(relative), false) =
            (self.capture, relative.as_ref(), entry.is_dir)
        {
            let capture_limit = capture.get(relative).copied();
            self.metadata(128 + relative.as_os_str().len())?;
            let mut proof = MemberProofWriter {
                hash: Sha256::new(),
                prefix: Vec::new(),
                limit: capture_limit.unwrap_or_default(),
            };
            copy_bounded(
                reader,
                &mut proof,
                &entry.path,
                entry.size,
                &mut self.budget,
                self.work,
            )?;
            self.written
                .push((relative.clone(), hex::encode(proof.hash.finalize())));
            if capture_limit.is_some() {
                self.captured.insert(
                    relative.clone(),
                    ArchiveMemberPrefix {
                        truncated: entry.size > proof.prefix.len() as u64,
                        prefix: proof.prefix,
                    },
                );
            }
            self.entries.push(entry);
            return Ok(());
        }
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
        let mut writer = BudgetedWriter {
            inner: writer,
            disk: self.disk.as_mut(),
        };
        copy_bounded(
            reader,
            &mut writer,
            &entry.path,
            entry.size,
            &mut self.budget,
            self.work,
        )?;
        writer.flush()?;
        if let Some((path, file)) = output {
            self.written.push((path, hex::encode(file.hash.finalize())));
        }
        self.entries.push(entry);
        Ok(())
    }
}

struct BudgetedWriter<'a, 'disk> {
    inner: &'a mut dyn Write,
    disk: Option<&'a mut safe_fs::DiskBudget<'disk>>,
}
impl Write for BudgetedWriter<'_, '_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(disk) = &mut self.disk {
            disk.charge(buf.len() as u64)
                .map_err(std::io::Error::other)?;
        }
        self.inner.write_all(buf)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
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

struct MemberProofWriter {
    hash: Sha256,
    prefix: Vec<u8>,
    limit: usize,
}

impl Write for MemberProofWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.hash.update(bytes);
        let take = self
            .limit
            .saturating_sub(self.prefix.len())
            .min(bytes.len());
        self.prefix.extend_from_slice(&bytes[..take]);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn process_zip(file: &mut (impl Read + Seek), processing: &mut Processing<'_>) -> AppResult<()> {
    let (mut zip, count) = archive_zip::open(file)?;
    let central = zip.central_directory_start();
    let mut ranges = Vec::with_capacity(count);
    let mut entries = Vec::new();
    for i in 0..zip.len() {
        let entry = zip.by_index_raw(i)?;
        let name = entry.name()?;
        check_mode(entry.unix_mode().unwrap_or(0), entry.is_dir(), &name)?;
        let end = entry
            .data_start()
            .filter(|start| *start > entry.header_start())
            .and_then(|start| start.checked_add(entry.compressed_size()))
            .filter(|end| *end <= central)
            .ok_or_else(|| unsafe_archive("ZIP 条目超出压缩数据范围"))?;
        ranges.push((entry.header_start(), end));
        entries.push(ArchiveEntry {
            path: name.into_owned(),
            is_dir: entry.is_dir(),
            size: entry.size(),
            compressed_size: entry.compressed_size(),
        });
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(unsafe_archive("ZIP 条目压缩数据重叠"));
    }
    check_declared(&entries, processing.budget.limit)?;
    for (i, entry) in entries.into_iter().enumerate() {
        processing.entry(entry, &mut zip.by_index(i)?)?;
    }
    Ok(())
}

fn process_7z(
    file: &mut (impl Read + Seek),
    length: u64,
    processing: &mut Processing<'_>,
    dictionary_kib: usize,
) -> AppResult<()> {
    let password = sevenz_rust2::Password::empty();
    let archive = {
        let mut header = SevenZHeaderInput::new(&mut *file);
        let parsed = sevenz_rust2::Archive::read_with_memory_limit(
            &mut header,
            &password,
            SEVENZ_HEADER_KIB,
        );
        header.check().map_err(|e| unsafe_archive(e.to_string()))?;
        parsed.map_err(|e| unsafe_archive(format!("7z 头或字典超出解析能力: {e}")))?
    };
    archive
        .pack_sizes()
        .iter()
        .try_fold(archive.pack_pos().saturating_add(32), |offset, size| {
            offset.checked_add(*size).filter(|end| *end <= length)
        })
        .ok_or_else(|| unsafe_archive("7z 压缩流位于归档范围之外"))?;
    let convert = |entry: &sevenz_rust2::ArchiveEntry| -> AppResult<ArchiveEntry> {
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
    let entries = archive
        .files
        .iter()
        .map(convert)
        .collect::<AppResult<Vec<_>>>()?;
    check_declared(&entries, processing.budget.limit)?;
    let mut unpacked = 0u64;
    for block in &archive.blocks {
        // LZMA/LZMA2/BCJ(2) and BZip2 are decoded on one thread. The library
        // limits each dictionary, not their sum; reject exotic stacks before
        // they can combine many dictionaries or decoded intermediate streams.
        let size = block.get_unpack_size();
        let lzma_count = block
            .coders
            .iter()
            .filter(|coder| {
                matches!(
                    coder.encoder_method_id(),
                    [0x21] | [0x03, 0x01, 0x01] | [0x04, 0x02, 0x02]
                )
            })
            .count();
        if block.coders.len() > 8
            || block
                .coders
                .iter()
                .enumerate()
                .any(|(i, _)| block.get_unpack_size_at_index(i) > size)
        {
            return Err(unsafe_archive("不支持具有额外解压工作量的 7z 编码结构"));
        }
        if lzma_count > 4 {
            return Err(unsafe_archive("7z 解码器组合超过内存预算"));
        }
        unpacked = unpacked
            .checked_add(size)
            .ok_or_else(|| unsafe_archive("7z 解压流大小溢出"))?;
    }
    let declared = entries
        .iter()
        .try_fold(0u64, |size, entry| size.checked_add(entry.size))
        .ok_or_else(|| unsafe_archive("7z 文件大小溢出"))?;
    if unpacked != declared {
        return Err(unsafe_archive("7z 解压流大小与文件大小不符"));
    }
    for (index, block) in archive.blocks.iter().enumerate() {
        processing.work.check()?;
        let dictionaries = block
            .coders
            .iter()
            .filter(|coder| {
                matches!(
                    coder.encoder_method_id(),
                    [0x21] | [0x03, 0x01, 0x01] | [0x04, 0x02, 0x02]
                )
            })
            .count()
            .max(1);
        let mut failure = None;
        let decoded = sevenz_rust2::BlockDecoder::new_with_memory_limit(
            1,
            dictionary_kib / dictionaries,
            index,
            &archive,
            &password,
            file,
        )
        .for_each_entries(&mut |entry, data| {
            if let Err(error) = convert(entry).and_then(|entry| processing.entry(entry, data)) {
                failure = Some(error);
                return Err(sevenz_rust2::Error::Other("entry rejected".into()));
            }
            Ok(true)
        });
        if let Some(error) = failure {
            return Err(error);
        }
        decoded.map_err(|e| unsafe_archive(format!("7z 解码或字典限制: {e}")))?;
    }
    for (entry, block) in entries
        .into_iter()
        .zip(&archive.stream_map.file_block_index)
    {
        if block.is_none() {
            processing.entry(entry, &mut std::io::empty())?;
        }
    }
    if processing.budget.total != declared || processing.entries.len() != archive.files.len() {
        return Err(unsafe_archive("7z 实际条目清单与声明不符"));
    }
    Ok(())
}

// TAR metadata is small, but the tar iterator buffers GNU/PAX records before
// it exposes the next member. Read the 512-byte framing ourselves so an
// untrusted extension cannot force a payload-sized allocation.
fn process_tar(file: &mut (impl Read + Seek), processing: &mut Processing<'_>) -> AppResult<()> {
    let mut magic = [0u8; 6];
    let n = file.read(&mut magic)?;
    file.rewind()?;
    let mut reader: Box<dyn Read + '_> = if magic[..n].starts_with(b"\x1f\x8b") {
        Box::new(flate2::read::MultiGzDecoder::new(file))
    } else if magic[..n].starts_with(b"BZh") {
        Box::new(bzip2::read::MultiBzDecoder::new(file))
    } else if magic[..n].starts_with(b"\xfd7zXZ\0") {
        return Err(AppError::Unsupported("暂不支持 tar.xz".into()));
    } else {
        Box::new(file)
    };
    process_tar_stream(&mut *reader, processing)
}

fn process_tar_stream(mut reader: &mut dyn Read, processing: &mut Processing<'_>) -> AppResult<()> {
    let mut framing = TAR_FRAMING_BYTES;
    let mut charge_framing = |bytes| {
        framing = framing
            .checked_sub(bytes)
            .ok_or_else(|| unsafe_archive("tar 扩展头与填充超过元数据预算"))?;
        Ok::<_, AppError>(())
    };
    let mut pending_name = None;
    let mut pending_size = None;
    let mut pax = false;
    let mut header = tar::Header::new_old();
    let mut padding = [0; 512];
    loop {
        processing.work.check()?;
        charge_framing(512)?;
        reader.read_exact(header.as_mut_bytes())?;
        let raw = header.as_bytes();
        if raw.iter().all(|&byte| byte == 0) {
            break;
        }
        let checksum = raw[..148]
            .iter()
            .chain(&raw[156..])
            .map(|&b| u32::from(b))
            .sum::<u32>()
            + 8 * 32;
        if header.cksum()? != checksum {
            return Err(unsafe_archive("tar 头校验和不符"));
        }
        let kind = header.entry_type();
        let size = if kind.is_gnu_longname() || kind.is_pax_local_extensions() {
            let size = header.entry_size()?;
            if size > MAX_TAR_EXTENSION {
                return Err(unsafe_archive("tar 扩展头超过元数据预算"));
            }
            charge_framing(size)?;
            let mut data = vec![0; size as usize];
            reader.read_exact(&mut data)?;
            if kind.is_gnu_longname() {
                if pending_name.is_some() {
                    return Err(unsafe_archive("tar 条目名称重复声明"));
                }
                if data.last() == Some(&0) {
                    data.pop();
                }
                pending_name = Some(data);
            } else {
                if pax {
                    return Err(unsafe_archive("tar PAX 头重复"));
                }
                pax = true;
                for extension in tar::PaxExtensions::new(&data) {
                    let extension = extension?;
                    match extension.key_bytes() {
                        b"path" if pending_name.is_none() => {
                            pending_name = Some(extension.value_bytes().to_vec())
                        }
                        b"size" if pending_size.is_none() => {
                            pending_size = Some(
                                extension
                                    .value()
                                    .ok()
                                    .and_then(|x| x.parse::<u64>().ok())
                                    .ok_or_else(|| unsafe_archive("tar PAX 大小无效"))?,
                            );
                        }
                        b"path" | b"size" | b"linkpath" | b"SCHILY.realsize" => {
                            return Err(unsafe_archive("tar PAX 路径或大小重复/不受支持"))
                        }
                        key if key.starts_with(b"GNU.sparse") => {
                            return Err(unsafe_archive("不支持稀疏 tar 条目"))
                        }
                        _ => {}
                    }
                }
            }
            size
        } else {
            if !kind.is_file() && !kind.is_dir() {
                return Err(unsafe_archive("tar 链接、设备或特殊条目被拒绝"));
            }
            let path = pending_name
                .take()
                .unwrap_or_else(|| header.path_bytes().into_owned());
            let size = pending_size.take().unwrap_or(header.entry_size()?);
            pax = false;
            let path =
                String::from_utf8(path).map_err(|_| unsafe_archive("tar 条目名不是 UTF-8"))?;
            processing.entry(
                ArchiveEntry {
                    path,
                    is_dir: kind.is_dir(),
                    size,
                    compressed_size: 0,
                },
                &mut (&mut reader).take(size),
            )?;
            size
        };
        let padded = ((512 - size % 512) % 512) as usize;
        charge_framing(padded as u64)?;
        reader.read_exact(&mut padding[..padded])?;
        if padding[..padded].iter().any(|&byte| byte != 0) {
            return Err(unsafe_archive("tar 条目填充包含非零数据"));
        }
    }
    if pax || pending_name.is_some() || pending_size.is_some() {
        return Err(unsafe_archive("tar 扩展头没有对应条目"));
    }
    // Consume only real zero padding and validate the gzip/bzip2 trailer(s).
    let mut tail = 0;
    let mut buf = [0; 64 * 1024];
    loop {
        processing.work.check()?;
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        tail += n as u64;
        if tail > MAX_TAR_PADDING || buf[..n].iter().any(|&byte| byte != 0) {
            return Err(unsafe_archive("tar 结束标记后有非零数据或异常填充"));
        }
    }
    Ok(())
}

fn process<'a>(
    file: &mut File,
    format: &str,
    dest: Option<&'a StagingDirectory>,
    strip_root: Option<&'a str>,
    limit: u64,
    work: &'a Work<'a>,
) -> AppResult<Processing<'a>> {
    process_with_capture(
        file,
        format,
        Processing::new(dest, strip_root, limit, work)?,
        None,
        None,
    )
}

fn archive_slot(work: &Work<'_>) -> AppResult<std::sync::MutexGuard<'static, ()>> {
    // ZIP indexes and 7z dictionaries have bounded but nontrivial memory.
    // Do not let concurrent previews multiply that budget without bound.
    loop {
        work.check()?;
        match ARCHIVE_WORK.try_lock() {
            Ok(slot) => return Ok(slot),
            Err(std::sync::TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(Duration::from_millis(10))
            }
        }
    }
}

fn process_with_capture<'a>(
    file: &mut File,
    format: &str,
    processing: Processing<'a>,
    capture: Option<&'a BTreeMap<PathBuf, usize>>,
    expected: Option<&str>,
) -> AppResult<Processing<'a>> {
    let slot = archive_slot(processing.work)?;
    // RAR needs the CLI even for the read-only payload verification. Never
    // follow the source pathname again, and bind a verified request to the
    // digest of the actual bytes admitted to the child process.
    let snapshot = (format == "rar")
        .then(|| archive_extract::Snapshot::create(file, expected, processing.work))
        .transpose()?;
    process_locked(file, format, processing, capture, &slot, snapshot.as_ref())
}

fn process_locked<'a>(
    file: &mut File,
    format: &str,
    mut processing: Processing<'a>,
    capture: Option<&'a BTreeMap<PathBuf, usize>>,
    _slot: &std::sync::MutexGuard<'_, ()>,
    snapshot: Option<&archive_extract::Snapshot>,
) -> AppResult<Processing<'a>> {
    let work = processing.work;
    file.rewind()?;
    let length = file.metadata()?.len();
    processing.capture = capture;
    let result = if format == "rar" {
        let snapshot = snapshot.ok_or_else(|| unsafe_archive("缺少已验证的 RAR 副本"))?;
        archive_extract::process_rar(snapshot, &mut processing)
    } else {
        let mut input = CheckedInput::new(&mut *file, work, length);
        match format {
            "zip" => process_zip(&mut input, &mut processing),
            "7z" => process_7z(&mut input, length, &mut processing, SEVENZ_DICTIONARY_KIB),
            "tar" => process_tar(&mut input, &mut processing),
            _ => Err(unsafe_archive("不支持的归档格式")),
        }
    };
    work.check()?;
    result?;
    if file.metadata()?.len() != length {
        return Err(AppError::Conflict("归档文件长度在读取期间已变化".into()));
    }
    Ok(processing)
}

fn disk_space_for(path: &Path) -> AppResult<u64> {
    safe_fs::available_data_space(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )
}

pub fn list_zip(path: &Path) -> AppResult<Vec<ArchiveEntry>> {
    Ok(process(
        &mut safe_fs::open_regular(path)?,
        "zip",
        None,
        None,
        disk_space_for(path)?,
        &Work::new(&CancellationToken::new()),
    )?
    .entries)
}

pub fn list_7z(path: &Path) -> AppResult<Vec<ArchiveEntry>> {
    Ok(process(
        &mut safe_fs::open_regular(path)?,
        "7z",
        None,
        None,
        disk_space_for(path)?,
        &Work::new(&CancellationToken::new()),
    )?
    .entries)
}

pub fn list_tar_gz(path: &Path) -> AppResult<Vec<ArchiveEntry>> {
    Ok(process(
        &mut safe_fs::open_regular(path)?,
        "tar",
        None,
        None,
        disk_space_for(path)?,
        &Work::new(&CancellationToken::new()),
    )?
    .entries)
}

pub fn list_archive(path: &Path) -> AppResult<ArchiveListing> {
    list_archive_cancellable(path, &CancellationToken::new())
}

pub(crate) fn list_archive_cancellable(
    path: &Path,
    cancel: &CancellationToken,
) -> AppResult<ArchiveListing> {
    let mut file = safe_fs::open_regular(path)?;
    let format = detect_format_from(path, &mut file)?;
    let work = Work::new(cancel);
    build_listing(
        format,
        process(&mut file, format, None, None, disk_space_for(path)?, &work)?.entries,
    )
}

pub(crate) fn list_verified_archive(
    path: &Path,
    expected: &str,
    cancel: &CancellationToken,
) -> AppResult<ArchiveListing> {
    let work = Work::new(cancel);
    let mut file = safe_fs::open_regular(path)?;
    verify_open_file_with_work(&mut file, expected, &work)?;
    let format = detect_format_from(path, &mut file)?;
    let listing = build_listing(
        format,
        process_with_capture(
            &mut file,
            format,
            Processing::new(None, None, disk_space_for(path)?, &work)?,
            None,
            Some(expected),
        )?
        .entries,
    )?;
    verify_open_file_with_work(&mut file, expected, &work)?;
    Ok(listing)
}

pub(crate) struct ArchiveMemberPrefix {
    pub(crate) prefix: Vec<u8>,
    pub(crate) truncated: bool,
}

pub(crate) struct VerifiedArchiveMembers {
    pub(crate) listing: ArchiveListing,
    pub(crate) hashes: BTreeMap<PathBuf, String>,
    pub(crate) captured: BTreeMap<PathBuf, ArchiveMemberPrefix>,
}

/// Verify and hash all normalized regular members using the same parsers and
/// budgets as extraction, but never write. Optional text previews retain only
/// the requested prefixes; validation still consumes whole archive members.
pub(crate) fn verify_archive_members(
    path: &Path,
    expected: &str,
    capture: &BTreeMap<PathBuf, usize>,
    cancel: &CancellationToken,
) -> AppResult<VerifiedArchiveMembers> {
    const MAX_PREFIX_BYTES: usize = 512 * 1024;
    if capture.len() > 8
        || capture
            .values()
            .try_fold(0usize, |total, bytes| total.checked_add(*bytes))
            .is_none_or(|total| total > MAX_PREFIX_BYTES)
    {
        return Err(unsafe_archive("归档预览超过保留字节预算"));
    }
    let work = Work::new(cancel);
    let mut file = safe_fs::open_regular(path)?;
    safe_fs::verify_regular_at(&file, path)?;
    verify_open_file_with_work(&mut file, expected, &work)?;
    let format = detect_format_from(path, &mut file)?;
    let proof = process_with_capture(
        &mut file,
        format,
        Processing::new(None, None, disk_space_for(path)?, &work)?,
        Some(capture),
        Some(expected),
    )?;
    verify_open_file_with_work(&mut file, expected, &work)?;
    safe_fs::verify_regular_at(&file, path)?;
    if proof.captured.len() != capture.len() {
        return Err(AppError::NotFound("归档缺少请求的普通文件成员".into()));
    }
    Ok(VerifiedArchiveMembers {
        listing: build_listing(format, proof.entries)?,
        hashes: proof.written.into_iter().collect(),
        captured: proof.captured,
    })
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
/// A write failure discards the owned partial staging directory.
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
    let (listing, written, mut cleanup) = extract_open_file_with_work(
        file,
        format,
        dest,
        strip_root,
        None,
        &Work::new(&CancellationToken::new()),
    )?;
    cleanup.keep();
    Ok((listing, written))
}

fn extract_open_file_with_work(
    file: &mut File,
    format: &str,
    dest: &Path,
    strip_root: Option<&str>,
    expected: Option<&str>,
    work: &Work<'_>,
) -> AppResult<(ArchiveListing, WrittenFiles, safe_fs::StagingCleanup)> {
    let slot = archive_slot(work)?;
    let limit = disk_space_for(dest)?;
    // Give the CLI a private copy of the held file, not a path that can be
    // replaced between parsing and extraction. Verify the exact bytes of that
    // copy and retain per-member hashes as the contract for the CLI stream.
    let snapshot = archive_extract::Snapshot::create(file, expected, work)?;
    let mut input = File::open(&snapshot.path)?;
    let capture = BTreeMap::new();
    let preflight = process_locked(
        &mut input,
        format,
        Processing::new(None, strip_root, limit, work)?,
        Some(&capture),
        &slot,
        Some(&snapshot),
    )?;
    drop(input);
    let listing = build_listing(format, preflight.entries)?;
    let path_count = preflight.names.len() as u64;
    drop(preflight.names);
    let estimate = listing
        .total_size
        .checked_add(path_count.saturating_mul(4096))
        .ok_or_else(|| unsafe_archive("归档磁盘预算溢出"))?;
    if estimate > disk_space_for(dest)? {
        return Err(unsafe_archive("磁盘空间不足，无法创建解压暂存"));
    }
    work.check()?;
    // Release the held staging descriptor before attempting removal on Windows.
    let cleanup;
    let staging = StagingDirectory::create(dest)?;
    cleanup = staging.cleanup_guard()?;
    let mut output = Processing::new(Some(&staging), strip_root, limit, work)?;
    archive_extract::extract(&snapshot, format, &listing.entries, &mut output)?;
    let written = output.written;
    if listing.entries != output.entries
        || written.len() != preflight.written.len()
        || written
            .iter()
            .zip(&preflight.written)
            .any(|((path, hash), (relative, proof))| {
                path.strip_prefix(dest).ok() != Some(relative.as_path()) || hash != proof
            })
    {
        return Err(unsafe_archive(
            "7-Zip 解压结果与预检的文件清单或 SHA-256 不符",
        ));
    }
    if let Some(expected) = expected {
        verify_open_file_with_work(file, expected, work)?;
    }
    staging.verify_path()?;
    work.check()?;
    Ok((listing, written, cleanup))
}

pub(crate) struct VerifiedExtraction {
    pub(crate) listing: ArchiveListing,
    pub(crate) files: BTreeMap<PathBuf, String>,
    pub(crate) cleanup: safe_fs::StagingCleanup,
}

#[cfg(test)]
pub(crate) fn extract_verified_archive(
    path: &Path,
    dest: &Path,
    expected_sha256: &str,
) -> AppResult<VerifiedExtraction> {
    let mut extraction = extract_verified_archive_cancellable(
        path,
        dest,
        expected_sha256,
        &CancellationToken::new(),
    )?;
    extraction.cleanup.keep();
    Ok(extraction)
}

pub(crate) fn extract_verified_archive_cancellable(
    path: &Path,
    dest: &Path,
    expected_sha256: &str,
    cancel: &CancellationToken,
) -> AppResult<VerifiedExtraction> {
    let work = Work::new(cancel);
    let mut file = safe_fs::open_regular(path)?;
    verify_open_file_with_work(&mut file, expected_sha256, &work)?;
    let format = detect_format_from(path, &mut file)?;
    let (listing, written, cleanup) =
        extract_open_file_with_work(&mut file, format, dest, None, Some(expected_sha256), &work)?;
    Ok(VerifiedExtraction {
        listing,
        files: written.into_iter().collect(),
        cleanup,
    })
}

#[cfg(all(test, unix))]
fn verify_open_file(file: &mut File, expected: &str) -> AppResult<()> {
    verify_open_file_with_work(file, expected, &Work::new(&CancellationToken::new()))
}

pub(crate) fn hash_file_cancellable(path: &Path, cancel: &CancellationToken) -> AppResult<String> {
    hash_open_file(&mut safe_fs::open_regular(path)?, &Work::new(cancel))
}

fn hash_open_file(file: &mut File, work: &Work<'_>) -> AppResult<String> {
    let expected = file.metadata()?.len();
    file.rewind()?;
    let mut digest = Sha256::new();
    let mut buf = [0; 64 * 1024];
    let mut remaining = expected;
    let changed = || AppError::Conflict("载荷文件长度在完整性校验期间已变化".into());
    while remaining > 0 {
        work.check()?;
        let len = remaining.min(buf.len() as u64) as usize;
        let n = file.read(&mut buf[..len])?;
        if n == 0 {
            return Err(changed());
        }
        remaining -= n as u64;
        digest.update(&buf[..n]);
    }
    work.check()?;
    if file.read(&mut buf[..1])? != 0 || file.metadata()?.len() != expected {
        return Err(changed());
    }
    file.rewind()?;
    Ok(hex::encode(digest.finalize()))
}

fn verify_open_file_with_work(file: &mut File, expected: &str, work: &Work<'_>) -> AppResult<()> {
    if !hash_open_file(file, work)?.eq_ignore_ascii_case(expected) {
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
    fn high_ratio_zip_is_admitted_only_with_payload_budget() {
        let dir = fixture_dir();
        let path = dir.path().join("bomb.zip");
        zip_file(&path, &[("bomb", &vec![0; 1024 * 1024])], true);
        assert_eq!(list_archive(&path).unwrap().total_size, 1024 * 1024);
        let cancel = CancellationToken::new();
        assert!(process(
            &mut File::open(&path).unwrap(),
            "zip",
            None,
            None,
            1024,
            &Work::new(&cancel)
        )
        .is_err());
        let dest = dir.path().join("out");
        assert_eq!(
            extract_archive(&path, &dest, None).unwrap(),
            [dest.join("bomb")]
        );
    }

    #[test]
    fn actual_reads_enforce_declared_size_and_disk_budget() {
        let cancel = CancellationToken::new();
        let work = Work::new(&cancel);
        for (declared, total_limit) in [(9, 8), (8, 100), (10, 100)] {
            let mut output = Vec::new();
            assert!(copy_bounded(
                &mut &b"123456789"[..],
                &mut output,
                "entry",
                declared,
                &mut ReadBudget {
                    total: 0,
                    limit: total_limit
                },
                &work
            )
            .is_err());
            assert!(output.len() as u64 <= declared.min(total_limit));
        }
        let mut budget = ReadBudget {
            total: 99,
            limit: 100,
        };
        assert!(copy_bounded(
            &mut &b"ab"[..],
            &mut std::io::sink(),
            "second",
            2,
            &mut budget,
            &work
        )
        .is_err());
        assert_eq!(budget.total, 99);
    }

    #[test]
    fn entry_sizes_above_four_gib_are_counted_as_u64() {
        const SIZE: u64 = 4 * 1024 * 1024 * 1024 + 17;
        let entry = ArchiveEntry {
            path: "large.bin".into(),
            is_dir: false,
            size: SIZE,
            compressed_size: SIZE,
        };
        assert!(check_declared(&[entry], SIZE).is_ok());
        // Synthesize repeated bytes directly into the bounded reader; no giant fixture or disk write.
        let cancel = CancellationToken::new();
        let mut budget = ReadBudget {
            total: 0,
            limit: SIZE,
        };
        copy_bounded(
            &mut std::io::repeat(0).take(SIZE),
            &mut std::io::sink(),
            "large.bin",
            SIZE,
            &mut budget,
            &Work::new(&cancel),
        )
        .unwrap();
        assert_eq!(budget.total, SIZE);
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
            ("huge", tar::EntryType::Regular, 4 * 1024 * 1024 * 1024 + 1),
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
        assert_eq!(list_archive(&path).unwrap().total_size, 1024 * 1024);
        // tar iteration stops at the end marker; the gzip stream must still
        // be drained under its budget, including concatenated gzip members.
        tar_file(&path, "small", tar::EntryType::Regular, 0, b"", true);
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        std::io::copy(
            &mut std::io::repeat(0).take(MAX_TAR_PADDING + 1),
            &mut encoder,
        )
        .unwrap();
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&encoder.finish().unwrap())
            .unwrap();
        assert!(list_archive(&path).is_err());
    }

    fn sevenz_file(path: &Path, name: &str, attributes: Option<u32>, data: &[u8]) {
        let mut writer = sevenz_rust2::ArchiveWriter::create(path).unwrap();
        let mut entry = sevenz_rust2::ArchiveEntry::new();
        entry.name = name.into();
        entry.has_windows_attributes = attributes.is_some();
        entry.windows_attributes = attributes.unwrap_or(0);
        writer.push_archive_entry(entry, Some(data)).unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn solid_sevenz_is_streamed_and_requires_a_bounded_dictionary() {
        let dir = fixture_dir();
        let path = dir.path().join("solid.7z");
        let data = vec![0u8; 512 * 1024];
        let mut writer = sevenz_rust2::ArchiveWriter::create(&path).unwrap();
        writer
            .push_archive_entries(
                ["left.bin", "right.bin"]
                    .into_iter()
                    .map(sevenz_rust2::ArchiveEntry::new_file)
                    .collect(),
                [data.as_slice(), data.as_slice()]
                    .into_iter()
                    .map(sevenz_rust2::SourceReader::new)
                    .collect(),
            )
            .unwrap();
        writer.finish().unwrap();
        let listing = list_archive(&path).unwrap();
        assert_eq!(listing.total_size, 2 * data.len() as u64);
        assert_eq!(listing.entries.len(), 2);
        let mut file = File::open(&path).unwrap();
        let length = file.metadata().unwrap().len();
        let cancel = CancellationToken::new();
        let work = Work::new(&cancel);
        let mut processing = Processing::new(None, None, u64::MAX, &work).unwrap();
        let err = process_7z(&mut file, length, &mut processing, 1).unwrap_err();
        assert!(err.to_string().contains("7z 解码或字典限制"), "{err}");
        assert!(processing.entries.is_empty());
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
    fn seven_zip_stream_assigns_content_to_the_preflight_member() {
        let temp = fixture_dir();
        let zip = temp.path().join("测试 归档.zip");
        // Same length, deliberately not alphabetical: checking size or exit
        // status alone would not catch a swapped stream.
        zip_file(&zip, &[("Z/data", b"last"), ("A/data", b"1st!")], true);
        let dest = temp.path().join("out");
        extract_archive(&zip, &dest, None).unwrap();
        assert_eq!(std::fs::read(dest.join("Z/data")).unwrap(), b"last");
        assert_eq!(std::fs::read(dest.join("A/data")).unwrap(), b"1st!");
    }

    #[test]
    fn seven_zip_extracts_bzip2_tar_into_guarded_directories() {
        let temp = fixture_dir();
        let tar = temp.path().join("raw.tar");
        tar_file(
            &tar,
            "folder/file",
            tar::EntryType::Regular,
            4,
            b"data",
            false,
        );
        let path = temp.path().join("file.tar.bz2");
        let mut encoder =
            bzip2::write::BzEncoder::new(File::create(&path).unwrap(), bzip2::Compression::fast());
        std::io::copy(&mut File::open(tar).unwrap(), &mut encoder).unwrap();
        encoder.finish().unwrap();
        let dest = temp.path().join("out");
        extract_archive(&path, &dest, Some("folder")).unwrap();
        assert_eq!(std::fs::read(dest.join("file")).unwrap(), b"data");
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
        sevenz_file(&path, "compressed", None, &vec![0; 1024 * 1024]);
        assert_eq!(list_archive(&path).unwrap().total_size, 1024 * 1024);
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
    fn archive_progress_checks_real_extent_replays_and_cancellation() {
        let token = CancellationToken::new();
        let work = Work::new(&token);
        let mut input = CheckedInput::new(std::io::Cursor::new(b"123456"), &work, 4);
        input.remaining_io = 6;
        let mut data = [0; 8];
        assert_eq!(input.read(&mut data).unwrap(), 4);
        assert_eq!(&data[..4], b"1234");
        assert_eq!(input.read(&mut data).unwrap(), 0);
        assert_eq!(input.seek(SeekFrom::End(0)).unwrap(), 4);
        assert!(input.seek(SeekFrom::End(1)).is_err());
        input.rewind().unwrap();
        assert_eq!(input.read(&mut data).unwrap(), 2);
        assert!(input
            .read(&mut data)
            .unwrap_err()
            .to_string()
            .contains("重复读取超过字节预算"));
        token.cancel();
        assert!(matches!(work.check(), Err(AppError::Cancelled)));
        assert!(input.rewind().unwrap_err().to_string().contains("已取消"));
    }

    #[test]
    fn zip_rejects_overlapping_member_data_in_preflight() {
        let dir = fixture_dir();
        let path = dir.path().join("overlap.zip");
        zip_file(&path, &[("a", b"same"), ("b", b"same")], false);
        let mut bytes = std::fs::read(&path).unwrap();
        let records: Vec<_> = bytes
            .windows(4)
            .enumerate()
            .filter_map(|(i, w)| (w == b"PK\x01\x02").then_some(i))
            .collect();
        let offset: [u8; 4] = bytes[records[0] + 42..records[0] + 46].try_into().unwrap();
        bytes[records[1] + 42..records[1] + 46].copy_from_slice(&offset);
        std::fs::write(&path, bytes).unwrap();
        let dest = dir.path().join("out");
        assert!(extract_archive(&path, &dest, None).is_err());
        assert!(!dest.exists());
    }

    #[test]
    fn zip64_directory_above_one_hundred_thousand_entries_is_admitted() {
        let dir = fixture_dir();
        let path = dir.path().join("many.zip");
        let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for index in 0..100_001 {
            writer.start_file(format!("m{index:06}"), options).unwrap();
        }
        writer.finish().unwrap();
        let (zip, count) = archive_zip::open(&mut File::open(&path).unwrap())
            .map(|(zip, count)| (zip.len(), count))
            .unwrap();
        assert_eq!((zip, count), (100_001, 100_001));
    }

    #[test]
    fn verified_member_previews_keep_only_requested_prefixes() {
        let dir = fixture_dir();
        let path = dir.path().join("mod.zip");
        zip_file(
            &path,
            &[("nested/README.txt", b"review"), ("payload", b"full")],
            true,
        );
        let cancel = CancellationToken::new();
        let digest = hash_file_cancellable(&path, &cancel).unwrap();
        let member = Path::new("nested").join("README.txt");
        let selected = BTreeMap::from([(member.clone(), 3)]);
        let output = verify_archive_members(&path, &digest, &selected, &cancel).unwrap();
        assert_eq!(output.listing.total_size, 10);
        assert_eq!(output.hashes.len(), 2);
        assert_eq!(
            output.hashes[&member],
            hex::encode(Sha256::digest(b"review"))
        );
        assert!(output.captured[&member].truncated);
        assert_eq!(output.captured[&member].prefix, b"rev");
        assert_eq!(output.captured.len(), 1);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn extraction_cancellation_and_final_hash_failure_do_not_leave_staging() {
        let dir = fixture_dir();
        let path = dir.path().join("mod.zip");
        let dest = dir.path().join("out");
        zip_file(&path, &[("file.txt", b"valid")], false);
        let cancel = CancellationToken::new();
        let wrong = "00".repeat(32);
        assert!(matches!(
            extract_open_file_with_work(
                &mut File::open(&path).unwrap(),
                "zip",
                &dest,
                None,
                Some(&wrong),
                &Work::new(&cancel)
            ),
            Err(AppError::Conflict(_))
        ));
        assert!(!dest.exists());
        cancel.cancel();
        assert!(matches!(
            extract_verified_archive_cancellable(&path, &dest, &wrong, &cancel),
            Err(AppError::Cancelled)
        ));
        assert!(!dest.exists());
    }

    #[test]
    fn declaration_is_checked_against_exact_budget() {
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
            sevenz_rust2::ArchiveReader::open(&path, sevenz_rust2::Password::empty()).unwrap();
        assert!(reader.archive().files[0].is_anti_item);
        assert!(matches!(
            list_archive(&path),
            Err(AppError::UnsafeArchive(_))
        ));
    }
}
