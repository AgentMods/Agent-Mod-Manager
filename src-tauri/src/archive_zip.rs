//! Bound zip-rs' eager central-directory allocation and its fallback footer search.
use super::unsafe_archive;
use crate::error::AppResult;
use std::{
    cell::RefCell,
    io::{self, Read, Seek, SeekFrom},
    rc::Rc,
};

const EOCD: &[u8] = b"PK\x05\x06";
const ZIP64_END: &[u8] = b"PK\x06\x06";
const ZIP64_LOCATOR: &[u8] = b"PK\x06\x07";
const CENTRAL: &[u8] = b"PK\x01\x02";
const MAX_CENTRAL_BYTES: u64 = 32 * 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 128 * 1024 * 1024;

fn le16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn le64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
fn read_at(reader: &mut (impl Read + Seek), offset: u64, bytes: &mut [u8]) -> io::Result<()> {
    reader.seek(SeekFrom::Start(offset))?;
    reader.read_exact(bytes)
}

struct CentralMetadata {
    start: u64,
    bytes: Vec<u8>,
}

// zip-rs parses ZIP64 counts before reading any entries and searches the file
// for earlier EOCD candidates on failure. During metadata parsing expose only
// the already admitted directory and footer, then switch back to the same file.
pub(super) struct ZipView<'a, R> {
    reader: &'a mut R,
    metadata: Rc<RefCell<Option<CentralMetadata>>>,
    position: u64,
    length: u64,
}
impl<R: Read + Seek> Read for ZipView<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let meta = self.metadata.borrow();
        let Some(CentralMetadata { start, bytes }) = meta.as_ref() else {
            return self.reader.read(buf);
        };
        if self.position < *start {
            // The footer finder may read a window that starts before the CD.
            // Limit its scan; it cannot continue digging through payload bytes.
            if *start - self.position > 128 * 1024 {
                return Err(io::Error::other(
                    "ZIP footer search outside the metadata region",
                ));
            }
            let n = buf.len().min((*start - self.position) as usize);
            buf[..n].fill(0);
            self.position += n as u64;
            return Ok(n);
        }
        let offset = (self.position - start).min(bytes.len() as u64) as usize;
        let n = buf.len().min(bytes.len() - offset);
        buf[..n].copy_from_slice(&bytes[offset..offset + n]);
        self.position += n as u64;
        Ok(n)
    }
}
impl<R: Seek> Seek for ZipView<'_, R> {
    fn seek(&mut self, seek: SeekFrom) -> io::Result<u64> {
        if self.metadata.borrow().is_none() {
            return self.reader.seek(seek);
        }
        self.position = match seek {
            SeekFrom::Start(pos) => Some(pos),
            SeekFrom::End(relative) => self.length.checked_add_signed(relative),
            SeekFrom::Current(relative) => self.position.checked_add_signed(relative),
        }
        .ok_or_else(|| io::Error::other("invalid ZIP seek"))?;
        Ok(self.position)
    }
}

pub(super) fn open<R: Read + Seek>(
    reader: &mut R,
) -> AppResult<(zip::ZipArchive<ZipView<'_, R>>, usize)> {
    let invalid = || unsafe_archive("无效或不受支持的 ZIP 中央目录");
    let length = reader.seek(SeekFrom::End(0))?;
    let tail_len = length.min(22 + u64::from(u16::MAX));
    let tail_start = length - tail_len;
    let mut tail = vec![0; tail_len as usize];
    read_at(reader, tail_start, &mut tail)?;
    let index = tail
        .windows(4)
        .enumerate()
        .filter_map(|(i, w)| {
            (w == EOCD
                && i + 22 <= tail.len()
                && i + 22 + usize::from(le16(&tail, i + 20)) == tail.len())
            .then_some(i)
        })
        .next_back()
        .ok_or_else(invalid)?;
    let end_pos = tail_start + index as u64;
    let footer = &tail[index..];
    if le32(footer, 4) != 0 || le16(footer, 8) != le16(footer, 10) {
        return Err(invalid());
    }
    let mut locator = [0; 20];
    let locator_pos = end_pos.saturating_sub(20);
    let is_zip64 = end_pos >= 20 && {
        read_at(reader, locator_pos, &mut locator)?;
        locator.starts_with(ZIP64_LOCATOR)
    };
    let mut expected_zip64 = None;
    let (count, size, offset, central_end) = if is_zip64 {
        let position = le64(&locator, 8);
        let mut extended = [0; 56];
        if le32(&locator, 4) != 0
            || le32(&locator, 16) != 1
            || position.checked_add(56).is_none_or(|end| end > locator_pos)
        {
            return Err(invalid());
        }
        read_at(reader, position, &mut extended)?;
        let ext = le64(&extended, 4);
        if !extended.starts_with(ZIP64_END)
            || !(44..=64 * 1024).contains(&ext)
            || position.checked_add(ext + 12) != Some(locator_pos)
            || le64(&extended, 16) != 0
            || le64(&extended, 24) != le64(&extended, 32)
        {
            return Err(invalid());
        }
        expected_zip64 = Some(extended);
        (
            le64(&extended, 32),
            le64(&extended, 40),
            le64(&extended, 48),
            position,
        )
    } else {
        (
            u64::from(le16(footer, 10)),
            u64::from(le32(footer, 12)),
            u64::from(le32(footer, 16)),
            end_pos,
        )
    };
    let start = central_end.checked_sub(size).ok_or_else(invalid)?;
    if start < offset || (is_zip64 && start != offset) || count > size / 46 {
        return Err(invalid());
    }
    if size > MAX_CENTRAL_BYTES || count.saturating_mul(512).saturating_add(size) > MAX_INDEX_BYTES
    {
        return Err(unsafe_archive("ZIP 中央目录超过安全元数据预算"));
    }
    let mut metadata = vec![0; (length - start) as usize];
    read_at(reader, start, &mut metadata)?;
    if &metadata[(end_pos - start) as usize..] != footer {
        return Err(invalid());
    }
    if let Some(expected) = expected_zip64 {
        let extended = (central_end - start) as usize;
        let locator_start = (locator_pos - start) as usize;
        if metadata[extended..extended + expected.len()] != expected
            || metadata[locator_start..locator_start + locator.len()] != locator
        {
            return Err(invalid());
        }
    }
    // Do not permit zip-rs to fall back to another embedded footer, which
    // would have an unvalidated count or an attacker-chosen extension size.
    for (i, window) in metadata.windows(4).enumerate() {
        let position = start + i as u64;
        if (window == EOCD && position != end_pos)
            || (window == ZIP64_END && (!is_zip64 || position != central_end))
        {
            return Err(invalid());
        }
    }
    let mut rest = &metadata[..size as usize];
    for _ in 0..count {
        if rest.len() < 46 || !rest.starts_with(CENTRAL) {
            return Err(invalid());
        }
        let size = 46
            + usize::from(le16(rest, 28))
            + usize::from(le16(rest, 30))
            + usize::from(le16(rest, 32));
        rest = rest.get(size..).ok_or_else(invalid)?;
    }
    if !rest.is_empty() {
        return Err(invalid());
    }
    let metadata = Rc::new(RefCell::new(Some(CentralMetadata {
        start,
        bytes: metadata,
    })));
    let view = ZipView {
        reader,
        metadata: Rc::clone(&metadata),
        position: 0,
        length,
    };
    let zip = zip::ZipArchive::with_config(
        zip::read::Config {
            archive_offset: zip::read::ArchiveOffset::Known(start - offset),
        },
        view,
    )?;
    if zip.central_directory_start() != start || zip.len() as u64 != count {
        return Err(invalid());
    }
    metadata.borrow_mut().take();
    Ok((zip, count as usize))
}
