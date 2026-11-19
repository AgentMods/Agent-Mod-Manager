use crate::error::AppResult;
use std::{
    ffi::OsString,
    fs::File,
    io,
    mem::offset_of,
    os::windows::{ffi::OsStringExt, io::AsRawHandle},
};
use windows_sys::Win32::{
    Foundation::ERROR_NO_MORE_FILES,
    Storage::FileSystem::{
        FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo, GetFileInformationByHandleEx,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ID_BOTH_DIR_INFO,
        FILE_INFO_BY_HANDLE_CLASS,
    },
};

const BUFFER_SIZE: usize = 64 * 1024;
const RECORD_ALIGNMENT: usize = 8;
const NEXT_OFFSET: usize = offset_of!(FILE_ID_BOTH_DIR_INFO, NextEntryOffset);
const ATTRIBUTES_OFFSET: usize = offset_of!(FILE_ID_BOTH_DIR_INFO, FileAttributes);
const NAME_LENGTH_OFFSET: usize = offset_of!(FILE_ID_BOTH_DIR_INFO, FileNameLength);
const NAME_OFFSET: usize = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
const MIN_RECORD_SIZE: usize = NAME_OFFSET + 2;

#[repr(align(8))]
struct QueryBuffer([u8; BUFFER_SIZE]);

/// Enumerates a checked directory handle opened for listing. Do not enumerate
/// another clone concurrently: duplicated handles share enumeration state.
pub(super) fn entries(file: File) -> impl Iterator<Item = AppResult<(OsString, bool)>> {
    enumerate(move |class, buffer| {
        // SAFETY: the owned file keeps the handle live, and the output buffer is
        // writable, 8-byte aligned, and at least as large as the declared size.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                class,
                buffer.0.as_mut_ptr().cast(),
                BUFFER_SIZE as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })
}

fn enumerate(
    mut query: impl FnMut(FILE_INFO_BY_HANDLE_CLASS, &mut QueryBuffer) -> io::Result<()>,
) -> impl Iterator<Item = AppResult<(OsString, bool)>> {
    let mut buffer = Box::new(QueryBuffer([0; BUFFER_SIZE]));
    let mut class = FileIdBothDirectoryRestartInfo;
    let mut cursor = None;
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        loop {
            let offset = match cursor {
                Some(offset) => offset,
                None => {
                    // The API gives no byte count. Do not parse stale records from a
                    // previous batch if a malformed offset points into unwritten bytes.
                    buffer.0.fill(0);
                    if let Err(error) = query(class, &mut buffer) {
                        done = true;
                        return (error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32))
                            .then(|| Err(error.into()));
                    }
                    class = FileIdBothDirectoryInfo;
                    0
                }
            };
            match parse_record(&buffer.0[offset..]) {
                Ok(record) => {
                    cursor = record.next.map(|relative| offset + relative);
                    if let Some(entry) = record.entry {
                        return Some(Ok(entry));
                    }
                }
                Err(error) => {
                    done = true;
                    return Some(Err(error.into()));
                }
            }
        }
    })
}

struct Record {
    next: Option<usize>,
    entry: Option<(OsString, bool)>,
}

fn invalid_record() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Windows 目录枚举记录格式无效")
}

fn read_u32(bytes: &[u8], offset: usize) -> io::Result<u32> {
    bytes
        .get(offset..)
        .and_then(|tail| tail.first_chunk())
        .map(|word| u32::from_le_bytes(*word))
        .ok_or_else(invalid_record)
}

fn parse_record(bytes: &[u8]) -> io::Result<Record> {
    let name_len = read_u32(bytes, NAME_LENGTH_OFFSET)? as usize;
    if name_len == 0 || !name_len.is_multiple_of(2) {
        return Err(invalid_record());
    }
    let end = NAME_OFFSET
        .checked_add(name_len)
        .filter(|&end| end <= bytes.len())
        .ok_or_else(invalid_record)?;
    let next = match read_u32(bytes, NEXT_OFFSET)? as usize {
        0 => None,
        next if next >= end
            && next.is_multiple_of(RECORD_ALIGNMENT)
            && next <= bytes.len() - MIN_RECORD_SIZE =>
        {
            Some(next)
        }
        _ => return Err(invalid_record()),
    };

    let mut wide = Vec::with_capacity(name_len / 2);
    let (units, remainder) = bytes[NAME_OFFSET..end].as_chunks::<2>();
    debug_assert!(remainder.is_empty());
    for unit in units {
        let unit = u16::from_le_bytes(*unit);
        // Do not emit a NUL, a path separator, or a drive/stream delimiter.
        if matches!(u8::try_from(unit), Ok(0 | b'/' | b'\\' | b':')) {
            return Err(invalid_record());
        }
        wide.push(unit);
    }
    let dot = u16::from(b'.');
    let entry = if wide == [dot] || wide == [dot, dot] {
        None
    } else {
        let attributes = read_u32(bytes, ATTRIBUTES_OFFSET)?;
        let directory = attributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
            == FILE_ATTRIBUTE_DIRECTORY;
        Some((OsString::from_wide(&wide), directory))
    };
    Ok(Record { next, entry })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND};

    fn set_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn record(name: &[u16], attributes: u32) -> Vec<u8> {
        let mut bytes = vec![0; NAME_OFFSET + 2 * name.len()];
        set_u32(&mut bytes, ATTRIBUTES_OFFSET, attributes);
        set_u32(&mut bytes, NAME_LENGTH_OFFSET, (2 * name.len()) as u32);
        let (out_chunks, remainder) = bytes[NAME_OFFSET..].as_chunks_mut::<2>();
        debug_assert!(remainder.is_empty());
        for (out, unit) in out_chunks.iter_mut().zip(name) {
            out.copy_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    fn batch(records: impl IntoIterator<Item = Vec<u8>>) -> Vec<u8> {
        let mut records = records.into_iter().peekable();
        let mut bytes = Vec::new();
        while let Some(mut record) = records.next() {
            if records.peek().is_some() {
                let next = record.len().next_multiple_of(RECORD_ALIGNMENT);
                record.resize(next, 0);
                set_u32(&mut record, NEXT_OFFSET, next as u32);
            }
            bytes.extend(record);
        }
        bytes
    }

    #[test]
    fn rejects_out_of_bounds_and_overlapping_offsets() {
        let first = record(&[u16::from(b'a')], 0);
        let end = first.len();
        let mut bytes = batch([first, record(&[u16::from(b'b')], 0)]);
        let aligned = end.next_multiple_of(RECORD_ALIGNMENT);
        assert_eq!(parse_record(&bytes).unwrap().next, Some(aligned));
        for next in [
            1,
            end / RECORD_ALIGNMENT * RECORD_ALIGNMENT,
            aligned + 1,
            aligned + 8,
            u32::MAX as usize - 7,
        ] {
            set_u32(&mut bytes, NEXT_OFFSET, next as u32);
            assert!(parse_record(&bytes).is_err(), "accepted offset {next}");
        }
    }

    #[test]
    fn rejects_bad_name_lengths_and_path_delimiters() {
        let mut bytes = record(&[u16::from(b'a')], 0);
        assert!(parse_record(&bytes[..NAME_LENGTH_OFFSET + 3]).is_err());
        for length in [0, 1, 3, 4, u32::MAX - 1] {
            set_u32(&mut bytes, NAME_LENGTH_OFFSET, length);
            assert!(parse_record(&bytes).is_err(), "accepted length {length}");
        }
        for unit in [0, b'/' as u16, b'\\' as u16, b':' as u16] {
            assert!(parse_record(&record(&[unit], 0)).is_err());
        }
    }

    #[test]
    fn preserves_surrogate_pairs_and_lone_surrogates() {
        let wide = [0xD83D, 0xDE00, 0xD800, u16::from(b'a'), 0xDC00];
        let parsed = parse_record(&record(&wide, 0)).unwrap();
        let (name, is_dir) = parsed.entry.unwrap();
        assert_eq!(name.encode_wide().collect::<Vec<_>>(), wide);
        assert!(!is_dir);
        assert_eq!(parsed.next, None);
    }

    #[test]
    fn restarts_once_resumes_and_skips_dot_entries() {
        let dot = u16::from(b'.');
        let first = batch([
            record(&[dot], FILE_ATTRIBUTE_DIRECTORY),
            record(&[u16::from(b'd')], FILE_ATTRIBUTE_DIRECTORY),
            record(
                &[u16::from(b'r')],
                FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT,
            ),
        ]);
        let second = record(&[dot, dot], FILE_ATTRIBUTE_DIRECTORY);
        let mut calls = 0;
        let mut entries = enumerate(|class, buffer| {
            calls += 1;
            assert!(buffer.0.iter().all(|&byte| byte == 0));
            let data = match calls {
                1 => {
                    assert_eq!(class, FileIdBothDirectoryRestartInfo);
                    &first
                }
                2 => {
                    assert_eq!(class, FileIdBothDirectoryInfo);
                    &second
                }
                3 => {
                    assert_eq!(class, FileIdBothDirectoryInfo);
                    return Err(io::Error::from_raw_os_error(ERROR_NO_MORE_FILES as i32));
                }
                _ => panic!("queried past end of directory"),
            };
            buffer.0[..data.len()].copy_from_slice(data);
            Ok(())
        });
        assert_eq!(entries.next().unwrap().unwrap(), ("d".into(), true));
        assert_eq!(entries.next().unwrap().unwrap(), ("r".into(), false));
        assert!(entries.next().is_none());
        assert!(entries.next().is_none());
    }

    #[test]
    fn query_errors_are_yielded_once_without_retry() {
        for code in [ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND] {
            let mut calls = 0;
            let mut entries = enumerate(|_, _| {
                calls += 1;
                assert_eq!(calls, 1);
                Err(io::Error::from_raw_os_error(code as i32))
            });
            assert!(matches!(
                entries.next(),
                Some(Err(AppError::Io(error))) if error.raw_os_error() == Some(code as i32),
            ));
            assert!(entries.next().is_none());
        }
    }

    #[test]
    fn offset_into_unwritten_tail_yields_one_parse_error() {
        let mut bytes = record(&[u16::from(b'a')], 0);
        let next = bytes.len().next_multiple_of(RECORD_ALIGNMENT);
        set_u32(&mut bytes, NEXT_OFFSET, next as u32);
        let mut calls = 0;
        let mut entries = enumerate(|_, buffer| {
            calls += 1;
            assert_eq!(calls, 1);
            buffer.0[..bytes.len()].copy_from_slice(&bytes);
            Ok(())
        });
        assert_eq!(entries.next().unwrap().unwrap(), ("a".into(), false));
        assert!(matches!(
            entries.next(),
            Some(Err(AppError::Io(error))) if error.kind() == io::ErrorKind::InvalidData,
        ));
        assert!(entries.next().is_none());
    }
}
