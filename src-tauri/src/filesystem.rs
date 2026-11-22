use crate::{
    error::{AppError, AppResult},
    safe_fs,
    transaction::FileEngine,
    util,
};
use serde::Serialize;
use std::{
    collections::BinaryHeap,
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

pub(crate) const DIRECTORY_ENTRY_LIMIT: usize = 2000;
pub(crate) const TEXT_LINE_LIMIT: usize = 2000;
pub(crate) const TEXT_BYTE_LIMIT: usize = 256 * 1024;
// A single UTF-8 scalar can consume four bytes. Every valid page must advance.
pub(crate) const MIN_TEXT_BYTES: usize = 4;

#[derive(Debug, Serialize)]
pub struct TextLine {
    pub line: u64,
    pub text: String,
}
#[derive(Debug, Serialize)]
pub struct TextFile {
    pub lines: Vec<TextLine>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentDirectoryEntry {
    name: String,
    path: String,
    is_dir: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AgentPathRead {
    Directory(AgentDirectoryPage<AgentDirectoryEntry>),
    #[serde(untagged)]
    File(AgentTextPage),
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentDirectoryPage<T> {
    path: String,
    pub entries: Vec<T>,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_offset: Option<usize>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentTextPage {
    #[serde(flatten)]
    content: TextFile,
    #[serde(flatten)]
    pagination: Option<TextPagination>,
}

#[derive(Debug, Serialize)]
struct TextPagination {
    byte_offset: u64,
    end_byte_offset: u64,
    next_byte_offset: Option<u64>,
    starts_mid_line: bool,
    ends_mid_line: bool,
}

#[derive(Default, serde::Deserialize)]
pub(crate) struct DirectoryPageRequest {
    offset: Option<usize>,
    limit: Option<usize>,
}

impl DirectoryPageRequest {
    pub(crate) fn is_explicit(&self) -> bool {
        self.offset.is_some() || self.limit.is_some()
    }
}

#[derive(Default, serde::Deserialize)]
struct TextPageRequest {
    start_line: Option<u64>,
    byte_offset: Option<u64>,
    max_lines: Option<usize>,
    max_bytes: Option<usize>,
}

impl TextPageRequest {
    fn is_explicit(&self) -> bool {
        self.start_line.is_some()
            || self.byte_offset.is_some()
            || self.max_lines.is_some()
            || self.max_bytes.is_some()
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct AgentDirectoryRequest {
    path: PathBuf,
    #[serde(flatten)]
    pub page: DirectoryPageRequest,
}

#[derive(serde::Deserialize)]
pub(crate) struct AgentPathRequest {
    path: PathBuf,
    #[serde(flatten)]
    text: TextPageRequest,
    #[serde(flatten)]
    directory: DirectoryPageRequest,
}

pub fn read_dir(engine: &FileEngine, path: &Path) -> AppResult<Vec<util::DirEntryInfo>> {
    let path = engine.resolve_file_path(path)?;
    util::list_dir_shallow(&path, DIRECTORY_ENTRY_LIMIT)
}

pub fn read_file(engine: &FileEngine, path: &Path) -> AppResult<TextFile> {
    read_text(&engine.resolve_file_path(path)?)
}

pub(crate) fn read_agent_directory(
    engine: &FileEngine,
    request: &AgentDirectoryRequest,
    cancel: &CancellationToken,
) -> AppResult<AgentDirectoryPage<util::DirEntryInfo>> {
    let path = engine.resolve_file_path(&request.path)?;
    read_directory_page(&path, &request.page, cancel, |path, name, is_dir| {
        // Only read attributes, not file contents, and do not follow child links.
        let metadata = fs::symlink_metadata(path).ok();
        util::DirEntryInfo {
            path: crate::paths::display_path(path),
            name: name.to_string_lossy().into_owned(),
            is_dir: is_dir || metadata.as_ref().is_some_and(|meta| meta.is_dir()),
            size: metadata.as_ref().map_or(0, fs::Metadata::len),
            modified: metadata
                .and_then(|meta| meta.modified().ok())
                .map(|time| chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339())
                .unwrap_or_default(),
        }
    })
}

/// Resolve with the usual root and credential checks on every page, before classification.
pub(crate) fn read_agent_path(
    engine: &FileEngine,
    request: &AgentPathRequest,
    cancel: &CancellationToken,
) -> AppResult<AgentPathRead> {
    let path = engine.resolve_file_path(&request.path)?;
    if !fs::symlink_metadata(&path)?.is_dir() {
        if request.directory.is_explicit() {
            return Err(AppError::Invalid("offset 和 limit 仅适用于目录".into()));
        }
        return read_text_page(&path, &request.text, cancel).map(AgentPathRead::File);
    }
    if request.text.is_explicit() {
        return Err(AppError::Invalid("文本分页参数不能用于目录".into()));
    }
    read_directory_page(&path, &request.directory, cancel, |path, name, is_dir| {
        AgentDirectoryEntry {
            path: crate::paths::display_path(path),
            name: name.to_string_lossy().into_owned(),
            is_dir,
        }
    })
    .map(AgentPathRead::Directory)
}

fn require_active(cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    Ok(())
}

fn read_directory_page<T>(
    path: &Path,
    request: &DirectoryPageRequest,
    cancel: &CancellationToken,
    mut entry: impl FnMut(&Path, &std::ffi::OsStr, bool) -> T,
) -> AppResult<AgentDirectoryPage<T>> {
    let limit = request.limit.unwrap_or(DIRECTORY_ENTRY_LIMIT);
    if !(1..=DIRECTORY_ENTRY_LIMIT).contains(&limit) {
        return Err(AppError::Invalid(format!(
            "目录 limit 必须在 1 到 {DIRECTORY_ENTRY_LIMIT} 之间"
        )));
    }
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(AppError::Invalid("路径不是目录".into()));
    }
    let changed = || {
        AppError::Conflict(format!(
            "目录对象已改变: {}",
            crate::paths::display_path(path)
        ))
    };
    let identity = safe_fs::identity(path)?.ok_or_else(changed)?;
    // Select the prefix of the globally sorted native names, not of a single
    // OS enumeration chunk. The first page retains at most one page in memory.
    let offset = request.offset.unwrap_or(0);
    let required = offset.saturating_add(limit);
    let mut prefix = BinaryHeap::new();
    let mut total = 0;
    for child in safe_fs::directory_entries(path, &identity)? {
        require_active(cancel)?;
        let child = child?;
        total += 1;
        if prefix.len() < required {
            prefix.push(child);
        } else if let Some(mut last) = prefix.peek_mut() {
            if child < *last {
                *last = child;
            }
        }
    }
    require_active(cancel)?;
    let end = total.min(required);
    let start = offset.min(end);
    let mut entries = Vec::with_capacity(end - start);
    if start < end {
        let children = prefix.into_sorted_vec();
        for (name, is_dir) in &children[start..end] {
            require_active(cancel)?;
            entries.push(entry(&path.join(name), name, *is_dir));
        }
    }
    if safe_fs::identity(path)?.as_ref() != Some(&identity) {
        return Err(changed());
    }
    let truncated = end < total;
    Ok(AgentDirectoryPage {
        path: crate::paths::display_path(path),
        entries,
        truncated,
        next_offset: truncated.then_some(end),
    })
}

fn require_no_nul(bytes: &[u8]) -> AppResult<()> {
    if bytes.contains(&0) {
        return Err(AppError::Invalid(
            "此文件是二进制内容，不能作为文本读取".into(),
        ));
    }
    Ok(())
}

struct TextPosition {
    offset: u64,
    line: u64,
    tail: [u8; 3], // previous bytes for UTF-8/CRLF boundary checks; zero-padded at BOF
}

fn text_start(
    reader: &mut impl BufRead,
    request: &TextPageRequest,
    cancel: &CancellationToken,
) -> AppResult<TextPosition> {
    let mut position = TextPosition {
        offset: 0,
        line: 1,
        tail: [0; 3],
    };
    let target_line = request.start_line.unwrap_or(1);
    loop {
        let max = match request.byte_offset {
            Some(offset) if offset > position.offset => offset - position.offset,
            Some(_) => break,
            None if position.line < target_line => u64::MAX,
            None => break,
        };
        require_active(cancel)?;
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            break;
        }
        let mut count = 0;
        for &byte in &bytes[..(max.min(bytes.len() as u64) as usize)] {
            count += 1;
            if byte == b'\n' {
                position.line += 1;
                if request.byte_offset.is_none() && position.line == target_line {
                    break;
                }
            }
        }
        require_no_nul(&bytes[..count])?;
        let remembered = count.min(position.tail.len());
        let tail_start = position.tail.len() - remembered;
        position.tail.rotate_left(remembered);
        position.tail[tail_start..].copy_from_slice(&bytes[count - remembered..count]);
        reader.consume(count);
        position.offset += count as u64;
    }
    Ok(position)
}

/// Return the start of the CRLF or UTF-8 decoding unit containing a split point.
/// Retain the same lossy decoding as the original text reader for malformed bytes.
fn text_boundary(bytes: &[u8], at: usize) -> usize {
    if bytes.get(at) == Some(&b'\n') && at > 0 && bytes[at - 1] == b'\r' {
        return at - 1;
    }
    if !bytes.get(at).is_some_and(|byte| byte & 0xc0 == 0x80) {
        return at;
    }
    for start in (at.saturating_sub(3)..at).rev() {
        let width = match bytes[start] {
            0x80..=0xbf => continue,
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => break,
        };
        let unit = &bytes[start..(start + width).min(bytes.len())];
        let decoded = match std::str::from_utf8(unit) {
            Ok(_) => unit.len(),
            Err(error) => error.error_len().unwrap_or(unit.len()),
        };
        if start + decoded > at {
            return start;
        }
        break;
    }
    at
}

fn read_text_page(
    path: &Path,
    request: &TextPageRequest,
    cancel: &CancellationToken,
) -> AppResult<AgentTextPage> {
    if request.start_line == Some(0)
        || (request.start_line.is_some() && request.byte_offset.is_some())
    {
        return Err(AppError::Invalid(
            "start_line 从 1 开始，不能与 byte_offset 同时使用".into(),
        ));
    }
    let max_lines = request.max_lines.unwrap_or(TEXT_LINE_LIMIT);
    if !(1..=TEXT_LINE_LIMIT).contains(&max_lines) {
        return Err(AppError::Invalid(format!(
            "max_lines 必须在 1 到 {TEXT_LINE_LIMIT} 之间"
        )));
    }
    let max_bytes = request.max_bytes.unwrap_or(TEXT_BYTE_LIMIT);
    if !(MIN_TEXT_BYTES..=TEXT_BYTE_LIMIT).contains(&max_bytes) {
        return Err(AppError::Invalid(format!(
            "max_bytes 必须在 {MIN_TEXT_BYTES} 到 {TEXT_BYTE_LIMIT} 之间"
        )));
    }
    let mut reader = BufReader::new(safe_fs::open_regular(path)?);
    let start = text_start(&mut reader, request, cancel)?;
    require_active(cancel)?;
    let context_len = start.tail.len();
    let mut bytes = Vec::with_capacity(context_len + max_bytes + 4);
    bytes.extend_from_slice(&start.tail);
    // Enough lookahead to detect EOF, CRLF and a split within a UTF-8 unit.
    reader
        .take((max_bytes + 4) as u64)
        .read_to_end(&mut bytes)?;
    if text_boundary(&bytes, context_len) != context_len {
        return Err(AppError::Invalid(
            "byte_offset 位于 UTF-8 字符或 CRLF 内部".into(),
        ));
    }
    let byte_end = text_boundary(&bytes, (context_len + max_bytes).min(bytes.len()));
    require_no_nul(&bytes[context_len..byte_end])?;
    let mut end = context_len;
    let lines = bytes[context_len..byte_end]
        .split_inclusive(|&byte| byte == b'\n')
        .take(max_lines)
        .enumerate()
        .map(|(index, raw)| {
            end += raw.len();
            let text = if let Some(line) = raw.strip_suffix(b"\n") {
                line.strip_suffix(b"\r").unwrap_or(line)
            } else {
                raw
            };
            TextLine {
                line: start.line + index as u64,
                text: String::from_utf8_lossy(text).into_owned(),
            }
        })
        .collect();
    let truncated = end < bytes.len();
    let end_byte_offset = start.offset + (end - context_len) as u64;
    Ok(AgentTextPage {
        content: TextFile { lines, truncated },
        pagination: (truncated || request.is_explicit()).then_some(TextPagination {
            byte_offset: start.offset,
            end_byte_offset,
            next_byte_offset: truncated.then_some(end_byte_offset),
            starts_mid_line: end > context_len && start.offset > 0 && start.tail[2] != b'\n',
            ends_mid_line: truncated && bytes[end - 1] != b'\n',
        }),
    })
}

fn read_text(path: &Path) -> AppResult<TextFile> {
    let mut bytes = Vec::new();
    safe_fs::open_regular(path)?
        .take(TEXT_BYTE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > TEXT_BYTE_LIMIT;
    bytes.truncate(TEXT_BYTE_LIMIT);
    if bytes.contains(&0) {
        return Err(AppError::Invalid(
            "此文件是二进制内容，不能作为文本读取".into(),
        ));
    }
    let content = String::from_utf8_lossy(&bytes);
    let line_truncated = content.lines().count() > TEXT_LINE_LIMIT;
    Ok(TextFile {
        lines: content
            .lines()
            .take(TEXT_LINE_LIMIT)
            .enumerate()
            .map(|(i, text)| TextLine {
                line: i as u64 + 1,
                text: text.into(),
            })
            .collect(),
        truncated: truncated || line_truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_numbered_text_and_registered_directory_reads() {
        let (_temp, engine, game) = crate::transaction::tests::setup();
        let file = game.join("log.txt");
        std::fs::write(&file, "first\nsecond\n").unwrap();
        let text = read_file(&engine, &file).unwrap();
        assert_eq!(text.lines[1].line, 2);
        assert_eq!(text.lines[1].text, "second");
        assert!(!text.truncated);
        assert_eq!(read_dir(&engine, &game).unwrap().len(), 1);
        std::fs::write(&file, "line\n".repeat(3000)).unwrap();
        let text = read_file(&engine, &file).unwrap();
        assert_eq!(text.lines.len(), 2000);
        assert!(text.truncated);
    }

    #[test]
    fn agent_directory_preview_pages_a_global_sort_past_the_old_total_limit() {
        let (_temp, engine, game) = crate::transaction::tests::setup();
        let engine = engine.for_local_agent();
        let cancel = CancellationToken::new();
        let read_page = |offset| {
            let AgentPathRead::Directory(page) = read_agent_path(
                &engine,
                &AgentPathRequest {
                    path: game.clone(),
                    text: TextPageRequest::default(),
                    directory: DirectoryPageRequest {
                        offset,
                        limit: None,
                    },
                },
                &cancel,
            )
            .unwrap() else {
                panic!("expected a directory preview");
            };
            page
        };
        let empty = read_page(None);
        assert!(empty.entries.is_empty());
        assert!(!empty.truncated);
        assert_eq!(empty.next_offset, None);

        for i in (0..=DIRECTORY_ENTRY_LIMIT).rev() {
            std::fs::write(game.join(format!("{i:04}.txt")), []).unwrap();
        }
        let first = read_page(None);
        assert_eq!(first.entries.len(), DIRECTORY_ENTRY_LIMIT);
        assert!(first.truncated);
        assert_eq!(first.next_offset, Some(DIRECTORY_ENTRY_LIMIT));
        let last = read_page(first.next_offset);
        assert!(!last.truncated);
        assert_eq!(last.next_offset, None);
        assert_eq!(last.entries.len(), 1);
        for (i, entry) in first.entries.into_iter().chain(last.entries).enumerate() {
            assert_eq!(entry.name, format!("{i:04}.txt"));
        }
        let past_end = read_page(Some(usize::MAX));
        assert!(past_end.entries.is_empty());
        assert_eq!(past_end.next_offset, None);
    }

    #[test]
    fn agent_text_fragments_preserve_crlf_and_lossy_utf8_at_byte_boundaries() {
        let (_temp, _engine, game) = crate::transaction::tests::setup();
        let file = game.join("mixed.txt");
        let raw = b"abc\xf0\x9f\x8c\xb1\r\n\r\nstray\r!\nabc\xe4\xb8\nend\r";
        std::fs::write(&file, raw).unwrap();
        let cancel = CancellationToken::new();
        for max_bytes in MIN_TEXT_BYTES..=10 {
            let mut offset = 0;
            let mut stitched = Vec::<String>::new();
            loop {
                let page = read_text_page(
                    &file,
                    &TextPageRequest {
                        byte_offset: Some(offset),
                        max_lines: Some(2),
                        max_bytes: Some(max_bytes),
                        ..Default::default()
                    },
                    &cancel,
                )
                .unwrap();
                let pagination = page.pagination.unwrap();
                assert_eq!(pagination.byte_offset, offset);
                assert!(pagination.end_byte_offset > offset);
                assert!(pagination.end_byte_offset - offset <= max_bytes as u64);
                for line in page.content.lines {
                    let index = (line.line - 1) as usize;
                    if index == stitched.len() {
                        stitched.push(line.text);
                    } else {
                        assert_eq!(index + 1, stitched.len());
                        stitched[index].push_str(&line.text);
                    }
                }
                let Some(next) = pagination.next_byte_offset else {
                    assert!(!page.content.truncated);
                    assert_eq!(pagination.end_byte_offset, raw.len() as u64);
                    break;
                };
                assert!(page.content.truncated);
                assert_eq!(next, pagination.end_byte_offset);
                offset = next;
            }
            assert_eq!(
                stitched,
                String::from_utf8_lossy(raw).lines().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn binary_and_outside_files_are_not_returned() {
        let (temp, engine, game) = crate::transaction::tests::setup();
        let binary = game.join("binary.dat");
        std::fs::write(&binary, b"a\0b").unwrap();
        assert!(read_file(&engine, &binary).is_err());
        let outside = temp.path().join("outside.txt");
        std::fs::write(&outside, b"private").unwrap();
        assert!(read_file(&engine, &outside).is_err());
        assert!(read_dir(&engine, temp.path()).is_err());
    }

    #[test]
    fn credentials_and_env_are_not_readable_by_file_tools() {
        let (_temp, engine, game) = crate::transaction::tests::setup();
        let private = engine.paths.root.join("private");
        std::fs::create_dir(&private).unwrap();
        for path in [
            private.join(".env"),
            private.join("credentials.json"),
            game.join(".env"),
        ] {
            std::fs::write(&path, "private-material").unwrap();
            assert!(read_file(&engine, &path).is_err());
            assert!(engine
                .plan_file_ops(
                    vec![crate::transaction::FileOp::Copy {
                        src: path,
                        dst: game.join("leak.txt")
                    }],
                    "must reject".into()
                )
                .is_err());
        }
        assert!(read_dir(&engine, &private).is_err());
    }
}
