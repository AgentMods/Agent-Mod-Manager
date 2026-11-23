//! Physical RAR 4.x / 5.x member inventory. Use the on-disk block sequence,
//! never a diagnostic listing or RAR5 Quick Open's untrusted header copies.
//! Layouts: RARLAB's technote and UnRAR headers; decoder: locked 7-Zip 26.06.

use super::{
    check_entry, check_mode, unsafe_archive, ArchiveEntry, METADATA_BYTES, RAR4_MAGIC, RAR5_MAGIC,
};
use crate::error::{AppError, AppResult};
use flate2::Crc;
use std::io::{Read, Seek, SeekFrom};

const MAX_DICTIONARY: u64 = 128 * 1024 * 1024;
const MAX_NAME: usize = 4096;

type Member = (ArchiveEntry, Option<u32>);

fn invalid() -> AppError {
    unsafe_archive("RAR 归档损坏或含有不受支持的结构")
}

fn multi_volume() -> AppError {
    unsafe_archive("不支持分卷 RAR 归档或跨卷条目")
}

fn encrypted() -> AppError {
    unsafe_archive("不支持加密 RAR 归档或加密条目")
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = Crc::new();
    crc.update(bytes);
    crc.sum()
}

fn take<'a>(rest: &mut &'a [u8], n: usize) -> AppResult<&'a [u8]> {
    let (part, tail) = rest.split_at_checked(n).ok_or_else(invalid)?;
    *rest = tail;
    Ok(part)
}

fn byte(rest: &mut &[u8]) -> AppResult<u8> {
    Ok(take(rest, 1)?[0])
}

fn word(rest: &mut &[u8]) -> AppResult<u16> {
    Ok(u16::from_le_bytes(take(rest, 2)?.try_into().unwrap()))
}

fn dword(rest: &mut &[u8]) -> AppResult<u32> {
    Ok(u32::from_le_bytes(take(rest, 4)?.try_into().unwrap()))
}

// RAR permits zero-extended vints (including a reserved-width Locator offset).
fn vint(rest: &mut &[u8]) -> AppResult<u64> {
    let mut value = 0u64;
    for shift in (0..=63).step_by(7) {
        let b = byte(rest)?;
        if shift == 63 && b > 1 {
            return Err(invalid());
        }
        value |= u64::from(b & 127) << shift;
        if b < 128 {
            return Ok(value);
        }
    }
    Err(invalid())
}

fn as_usize(n: u64) -> AppResult<usize> {
    usize::try_from(n).map_err(|_| invalid())
}

struct Scan<'a, R> {
    input: &'a mut R,
    length: u64,
    pos: u64,
    budget: usize,
    members: Vec<Member>,
}

impl<R: Read + Seek> Scan<'_, R> {
    fn end_of(&self, n: u64) -> AppResult<u64> {
        self.pos
            .checked_add(n)
            .filter(|&end| end <= self.length)
            .ok_or_else(invalid)
    }

    fn charge(&mut self, n: usize) -> AppResult<()> {
        self.budget = self
            .budget
            .checked_sub(n)
            .ok_or_else(|| unsafe_archive("RAR 归档元数据超过限制"))?;
        Ok(())
    }

    fn read(&mut self, bytes: &mut [u8]) -> AppResult<()> {
        let end = self.end_of(bytes.len() as u64)?;
        self.charge(bytes.len())?;
        self.input.read_exact(bytes)?;
        self.pos = end;
        Ok(())
    }

    fn read_header(&mut self, prefix: &[u8], len: usize) -> AppResult<Vec<u8>> {
        self.end_of(len as u64)?;
        if len > self.budget {
            return Err(unsafe_archive("RAR 归档元数据超过限制"));
        }
        let mut bytes = vec![0; prefix.len() + len];
        bytes[..prefix.len()].copy_from_slice(prefix);
        self.read(&mut bytes[prefix.len()..])?;
        Ok(bytes)
    }

    fn skip_data(&mut self, packed: u64) -> AppResult<()> {
        if packed != 0 {
            let end = self.end_of(packed)?;
            self.input.seek(SeekFrom::Start(end))?;
            self.pos = end;
        }
        Ok(())
    }

    fn push(&mut self, member: Member) -> AppResult<()> {
        // Charge name storage and geometric Vec growth as well as header input.
        self.charge(member.0.path.capacity() + 2 * std::mem::size_of::<Member>())?;
        self.members.push(member);
        Ok(())
    }
}

pub(super) fn inventory(
    input: &mut (impl Read + Seek),
    length: u64,
) -> AppResult<Vec<(ArchiveEntry, Option<u32>)>> {
    input.seek(SeekFrom::Start(0))?;
    let mut scan = Scan {
        input,
        length,
        pos: 0,
        budget: METADATA_BYTES,
        members: Vec::new(),
    };
    let mut magic = [0; 7];
    scan.read(&mut magic)?;
    if magic == RAR4_MAGIC {
        rar4(&mut scan)?;
    } else if magic == RAR5_MAGIC[..7] {
        scan.read(&mut magic[..1])?;
        if magic[0] != RAR5_MAGIC[7] {
            return Err(invalid());
        }
        rar5(&mut scan)?;
    } else {
        return Err(invalid());
    }
    if scan.pos != length {
        return Err(unsafe_archive("RAR 归档结束标记后存在额外数据"));
    }
    Ok(scan.members)
}

fn regular_member(
    path: String,
    size: u64,
    packed: u64,
    is_dir: bool,
    crc: Option<u32>,
) -> AppResult<Member> {
    if (is_dir || size == 0) && (packed != 0 || crc.is_some_and(|v| v != 0))
        || size != 0 && packed == 0
    {
        return Err(invalid());
    }
    let entry = ArchiveEntry {
        path,
        size,
        compressed_size: packed,
        is_dir,
    };
    check_entry(&entry)?;
    Ok((entry, crc))
}

fn windows_attributes(attributes: u64, is_dir: bool) -> AppResult<()> {
    // Ordinary DOS/Win32 attributes only: no label, device, reparse point,
    // encrypted attribute, Unix-mode extension or unknown high bits.
    if attributes & 0x400 != 0 {
        return Err(unsafe_archive("RAR 含有链接或重解析点"));
    }
    if attributes & !0x3bb7 != 0 || !is_dir && attributes & 0x10 != 0 {
        return Err(unsafe_archive("RAR 含有特殊或不受支持的文件属性"));
    }
    Ok(())
}

fn unix_attributes(attributes: u64, is_dir: bool, name: &str) -> AppResult<()> {
    let mode = u16::try_from(attributes).map_err(|_| invalid())?;
    check_mode(u32::from(mode), is_dir, name)
}

// A RAR4 Unicode name is UTF-8 if there is no separator NUL. Otherwise it
// contains an OEM fallback and a separate run-coded UTF-16 representation.
fn rar4_name(raw: &[u8], unicode: bool) -> AppResult<String> {
    if !unicode {
        // Legacy OEM code pages are platform-dependent in the actual decoder.
        if !raw.is_ascii() || raw.contains(&0) {
            return Err(invalid());
        }
    } else if let Some(i) = raw.iter().position(|&b| b == 0) {
        let mut enc = &raw[i + 1..];
        let high = u16::from(byte(&mut enc)?) << 8;
        let mut decoded = Vec::new();
        while !enc.is_empty() {
            let flags = byte(&mut enc)?;
            for shift in [6, 4, 2, 0] {
                let low = byte(&mut enc)?;
                match (flags >> shift) & 3 {
                    0 => decoded.push(u16::from(low)),
                    1 => decoded.push(high | u16::from(low)),
                    2 => decoded.push(u16::from_le_bytes([low, byte(&mut enc)?])),
                    _ => {
                        let correction = if low & 128 != 0 {
                            Some(byte(&mut enc)?)
                        } else {
                            None
                        };
                        let start = decoded.len();
                        let end = start + usize::from(low & 127) + 2;
                        let run = raw[..i].get(start..end).ok_or_else(invalid)?;
                        decoded.extend(run.iter().map(|&b| match correction {
                            Some(c) => high | u16::from(b.wrapping_add(c)),
                            None => u16::from(b),
                        }));
                    }
                }
                // 7-Zip reserves one code unit for a terminator in this buffer.
                if decoded.len() >= raw.len().min(0x400) {
                    return Err(invalid());
                }
                if enc.is_empty() {
                    break;
                }
            }
        }
        if decoded.is_empty() || decoded.contains(&0) {
            return Err(invalid());
        }
        return String::from_utf16(&decoded).map_err(|_| invalid());
    }
    utf8_name(raw).map(str::to_owned)
}

fn utf8_name(raw: &[u8]) -> AppResult<&str> {
    if raw.is_empty() || raw.len() > MAX_NAME || raw.contains(&0) {
        return Err(invalid());
    }
    std::str::from_utf8(raw).map_err(|_| invalid())
}

fn rar4(scan: &mut Scan<'_, impl Read + Seek>) -> AppResult<()> {
    let mut main = false;
    loop {
        let mut fixed = [0; 7];
        scan.read(&mut fixed)?;
        let mut prefix = &fixed[..];
        let expected = word(&mut prefix)?;
        let kind = byte(&mut prefix)?;
        let flags = word(&mut prefix)?;
        let size = usize::from(word(&mut prefix)?);
        let body_size = size.checked_sub(fixed.len()).ok_or_else(invalid)?;
        let header = scan.read_header(&fixed[2..], body_size)?;
        if crc32(&header) as u16 != expected {
            return Err(unsafe_archive("RAR4 归档头 CRC32 不匹配"));
        }
        let body = &header[5..];
        match (main, kind) {
            (false, 0x73) if flags & (1 | 0x100) != 0 => return Err(multi_volume()),
            (false, 0x73) if flags & 0x80 != 0 => return Err(encrypted()),
            (true, 0x7b) if flags & 9 != 0 => return Err(multi_volume()),
            (false, 0x73) if flags & !(0x04 | 0x08 | 0x10) == 0 && body == [0; 6] => main = true,
            (true, 0x74) => {
                let member = rar4_file(flags, body)?;
                scan.skip_data(member.0.compressed_size)?;
                scan.push(member)?;
            }
            (true, 0x7b)
                if flags & !0x4002 == 0 && body.len() == 4 * usize::from(flags & 2 != 0) =>
            {
                return Ok(());
            }
            _ => return Err(invalid()),
        }
    }
}

fn rar4_file(flags: u16, mut body: &[u8]) -> AppResult<Member> {
    if flags & 3 != 0 {
        return Err(multi_volume());
    }
    if flags & (4 | 0x400) != 0 {
        return Err(encrypted());
    }
    // Solid, dictionary/dir, 64-bit sizes, Unicode, ext. time, skip-if-unknown, long block.
    if flags & !0xd3f0 != 0 {
        return Err(invalid());
    }
    let mut packed = u64::from(dword(&mut body)?);
    let mut size = u64::from(dword(&mut body)?);
    let host = byte(&mut body)?;
    let crc = dword(&mut body)?;
    take(&mut body, 4)?; // DOS mtime
    let version = byte(&mut body)?;
    let method = byte(&mut body)?;
    let name_len = usize::from(word(&mut body)?);
    let attributes = u64::from(dword(&mut body)?);
    if flags & 0x100 != 0 {
        packed |= u64::from(dword(&mut body)?) << 32;
        size |= u64::from(dword(&mut body)?) << 32;
    } else if size == u64::from(u32::MAX) {
        return Err(invalid()); // Unknown size (original RAR without the large-file flag).
    }
    if name_len > MAX_NAME
        || size == u64::MAX
        || !(10..=40).contains(&version)
        || !(0x30..=0x35).contains(&method)
        || method == 0x30 && size != packed
    {
        return Err(invalid());
    }
    let name = rar4_name(take(&mut body, name_len)?, flags & 0x200 != 0)?;
    if flags & 0x1000 != 0 && !body.is_empty() {
        let time_flags = word(&mut body)?;
        if time_flags & 0xf != 0 {
            return Err(invalid());
        }
        for shift in [12, 8, 4] {
            let mode = (time_flags >> shift) & 0xf;
            if mode & 8 != 0 {
                take(
                    &mut body,
                    usize::from(mode & 3) + if shift == 12 { 0 } else { 4 },
                )?;
            }
        }
    }
    if !body.is_empty() {
        return Err(invalid());
    }
    let dictionary = (flags >> 5) & 7;
    // All non-directory RAR4 dictionary selectors are inherently <= 4 MiB.
    let mut is_dir = dictionary == 7;
    match host {
        0..=2 => {
            is_dir |= attributes & 0x10 != 0;
            windows_attributes(attributes, is_dir)?;
        }
        3 | 5 => {
            is_dir |= attributes & 0o170000 == 0o040000;
            unix_attributes(attributes, is_dir, &name)?;
        }
        _ => return Err(invalid()),
    }
    regular_member(name, size, packed, is_dir, Some(crc))
}

fn rar5_header(scan: &mut Scan<'_, impl Read + Seek>) -> AppResult<Vec<u8>> {
    let mut crc = [0; 4];
    scan.read(&mut crc)?;
    let mut size = [0; 3];
    for i in 0..size.len() {
        scan.read(&mut size[i..=i])?;
        if size[i] < 128 {
            let prefix = &size[..=i];
            let len = as_usize(vint(&mut &size[..=i])?)?;
            let header = scan.read_header(prefix, len)?;
            if crc32(&header) != u32::from_le_bytes(crc) {
                return Err(unsafe_archive("RAR5 归档头 CRC32 不匹配"));
            }
            // The size field is covered by CRC but excluded from the header body.
            return Ok(header);
        }
    }
    Err(invalid())
}

fn rar5(scan: &mut Scan<'_, impl Read + Seek>) -> AppResult<()> {
    let mut main = false;
    let mut services = 0;
    loop {
        let header = rar5_header(scan)?;
        let mut body = header.as_slice();
        vint(&mut body)?; // Header size (used and checked during read).
        let kind = vint(&mut body)?;
        let flags = vint(&mut body)?;
        // Extra, packed size, skip-if-unknown. Neither split nor child flags
        // are supported: the only permitted services are archive-wide.
        if kind == 4 {
            return Err(encrypted());
        }
        if flags & 0x18 != 0 {
            return Err(multi_volume());
        }
        if flags & !7 != 0 {
            return Err(invalid());
        }
        let extra_len = if flags & 1 != 0 {
            as_usize(vint(&mut body)?)?
        } else {
            0
        };
        let packed = if flags & 2 != 0 { vint(&mut body)? } else { 0 };
        let (mut body, extra) = body
            .split_at_checked(body.len().checked_sub(extra_len).ok_or_else(invalid)?)
            .ok_or_else(invalid)?;
        match (main, kind) {
            (false, 1) if flags & 2 == 0 => {
                // Solid, recovery record and lock flags don't alter membership.
                let archive_flags = vint(&mut body)?;
                if archive_flags & 3 != 0 {
                    return Err(multi_volume());
                }
                if archive_flags & !0x1c != 0 || !body.is_empty() {
                    return Err(invalid());
                }
                rar5_extras(extra, kind)?;
                main = true;
            }
            (true, 2 | 3) => {
                rar5_extras(extra, kind)?;
                let file = rar5_file(body, packed)?;
                if kind == 2 {
                    if file.name.contains(['\\', '\u{fffe}']) || services & 0b110 != 0 {
                        return Err(invalid());
                    }
                    match file.host {
                        0 => windows_attributes(file.attributes, file.is_dir)?,
                        1 => unix_attributes(file.attributes, file.is_dir, file.name)?,
                        _ => return Err(invalid()),
                    }
                    let member = regular_member(
                        file.name.to_owned(),
                        file.size,
                        packed,
                        file.is_dir,
                        file.crc,
                    )?;
                    scan.push(member)?;
                } else {
                    let service = match file.name {
                        "CMT" if scan.members.is_empty() && packed <= 256 * 1024 => 1,
                        "QO" => 2,
                        "RR" => 4,
                        _ => return Err(unsafe_archive("RAR5 含有不受支持的服务条目")),
                    };
                    if services & service != 0 || file.is_dir || file.method != 0 || file.solid {
                        return Err(invalid());
                    }
                    services |= service;
                    // Neither this inventory nor 7-Zip's RAR5 handler uses
                    // Locator offsets or Quick Open data to find file headers.
                    scan.charge(as_usize(packed)?)?;
                }
                scan.skip_data(packed)?;
            }
            (true, 5) if flags & 3 == 0 => {
                let end_flags = vint(&mut body)?;
                if end_flags & 1 != 0 {
                    return Err(multi_volume());
                }
                if end_flags != 0 || !body.is_empty() {
                    return Err(invalid());
                }
                return Ok(());
            }
            _ => return Err(invalid()),
        }
    }
}

struct File5<'a> {
    name: &'a str,
    host: u64,
    attributes: u64,
    size: u64,
    crc: Option<u32>,
    is_dir: bool,
    solid: bool,
    method: u64,
}

fn rar5_file(mut body: &[u8], packed: u64) -> AppResult<File5<'_>> {
    let flags = vint(&mut body)?;
    if flags & !7 != 0 {
        return Err(invalid()); // Includes unknown unpacked size.
    }
    let size = vint(&mut body)?;
    let attributes = vint(&mut body)?;
    if flags & 2 != 0 {
        take(&mut body, 4)?;
    }
    let crc = (flags & 4 != 0).then(|| dword(&mut body)).transpose()?;
    let compression = vint(&mut body)?;
    let version = compression & 0x3f;
    let method = (compression >> 7) & 7;
    if !matches!(version, 0 | 1)
        || method > 5
        || method == 0 && packed != size
        || (version == 0 && compression >> 14 != 0)
        || compression >> 21 != 0
    {
        return Err(invalid());
    }
    let exponent = (compression >> 10) & 31;
    let fraction = (compression >> 15) & 31;
    let dictionary = (32 + fraction) << (12 + exponent);
    if dictionary > MAX_DICTIONARY {
        return Err(unsafe_archive("RAR5 解压字典超过限制"));
    }
    let host = vint(&mut body)?;
    let len = as_usize(vint(&mut body)?)?;
    let name = utf8_name(take(&mut body, len)?)?;
    if !body.is_empty() {
        return Err(invalid());
    }
    Ok(File5 {
        name,
        host,
        attributes,
        size,
        crc,
        is_dir: flags & 1 != 0,
        solid: compression & 0x40 != 0,
        method,
    })
}

fn rar5_extras(mut extra: &[u8], kind: u64) -> AppResult<()> {
    let mut seen = 0u8;
    while !extra.is_empty() {
        let len = as_usize(vint(&mut extra)?)?;
        let mut record = take(&mut extra, len)?;
        let id = vint(&mut record)?;
        if !(1..=7).contains(&id) || seen & (1 << id) != 0 {
            return Err(invalid());
        }
        seen |= 1 << id;
        match (kind, id) {
            (1, 1) => {
                let flags = vint(&mut record)?;
                if flags & !3 != 0 {
                    return Err(invalid());
                }
                for _ in 0..flags.count_ones() {
                    vint(&mut record)?; // Ignore (possibly zero) Locator offsets.
                }
            }
            (1, 2) => {
                let flags = vint(&mut record)?;
                if flags & !15 != 0 {
                    return Err(invalid());
                }
                if flags & 1 != 0 {
                    let len = as_usize(vint(&mut record)?)?;
                    take(&mut record, len)?; // Original archive name, never a member path.
                }
                if flags & 2 != 0 {
                    take(&mut record, if flags & 12 == 4 { 4 } else { 8 })?;
                }
            }
            (2 | 3, 2) if vint(&mut record)? == 0 => {
                take(&mut record, 32)?; // BLAKE2sp, not a substitute for a CRC32.
            }
            (2 | 3, 3) => {
                let flags = vint(&mut record)?;
                if flags & !31 != 0 || flags & 17 == 16 {
                    return Err(invalid());
                }
                let width = if flags & 17 == 1 { 4 } else { 8 };
                take(&mut record, (flags & 14).count_ones() as usize * width)?;
            }
            (2, 6) => {
                let flags = vint(&mut record)?;
                if flags & !15 != 0 {
                    return Err(invalid());
                }
                for _ in 0..(flags & 3).count_ones() {
                    let len = as_usize(vint(&mut record)?)?;
                    take(&mut record, len)?;
                }
                for _ in 0..(flags & 12).count_ones() {
                    vint(&mut record)?;
                }
            }
            (2 | 3, 1) => return Err(encrypted()),
            (2 | 3, 5) => return Err(unsafe_archive("RAR 含有文件重定向或链接条目")),
            (3, 7) => record = &[], // Opaque data for a separately allowlisted service.
            // Crypto, versions (renamed by the decoder), redirects and unknown
            // record types are not safe to ignore.
            _ => return Err(invalid()),
        }
        if !record.is_empty() {
            return Err(invalid());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn var(mut n: u64, out: &mut Vec<u8>) {
        while n >= 128 {
            out.push(n as u8 | 128);
            n >>= 7;
        }
        out.push(n as u8);
    }

    fn r5_block(out: &mut Vec<u8>, kind: u64, flags: u64, body: &[u8], extra: &[u8], data: &[u8]) {
        let mut header = Vec::new();
        var(kind, &mut header);
        let flags = flags | u64::from(!extra.is_empty()) | (2 * u64::from(!data.is_empty()));
        var(flags, &mut header);
        if flags & 1 != 0 {
            var(extra.len() as u64, &mut header);
        }
        if flags & 2 != 0 {
            var(data.len() as u64, &mut header);
        }
        header.extend(body);
        header.extend(extra);
        let mut checked = Vec::new();
        var(header.len() as u64, &mut checked);
        checked.extend(header);
        out.extend(crc32(&checked).to_le_bytes());
        out.extend(checked);
        out.extend(data);
    }

    fn r5_file(
        name: &[u8],
        size: u64,
        attributes: u64,
        compression: u64,
        host: u8,
        flags: u8,
        crc: u32,
    ) -> Vec<u8> {
        let mut body = vec![flags];
        var(size, &mut body);
        var(attributes, &mut body);
        if flags & 2 != 0 {
            body.extend(0u32.to_le_bytes());
        }
        if flags & 4 != 0 {
            body.extend(crc.to_le_bytes());
        }
        var(compression, &mut body);
        body.push(host);
        var(name.len() as u64, &mut body);
        body.extend(name);
        body
    }

    fn r5_main(flags: u8, extra: &[u8]) -> Vec<u8> {
        let mut out = RAR5_MAGIC.to_vec();
        r5_block(&mut out, 1, 4, &[flags], extra, &[]);
        out
    }

    fn r5_end(out: &mut Vec<u8>, flags: u8) {
        r5_block(out, 5, 4, &[flags], &[], &[]);
    }

    fn parse(bytes: &[u8]) -> AppResult<Vec<Member>> {
        inventory(&mut Cursor::new(bytes), bytes.len() as u64)
    }

    fn r4_block(out: &mut Vec<u8>, kind: u8, flags: u16, body: &[u8], data: &[u8]) {
        let mut header = vec![kind];
        header.extend(flags.to_le_bytes());
        header.extend(((7 + body.len()) as u16).to_le_bytes());
        header.extend(body);
        out.extend((crc32(&header) as u16).to_le_bytes());
        out.extend(header);
        out.extend(data);
    }

    fn r4_file(name: &[u8], data: &[u8], host: u8, attributes: u32, flags: u16) -> Vec<u8> {
        let mut body = Vec::new();
        for _ in 0..2 {
            body.extend((data.len() as u32).to_le_bytes());
        }
        body.push(host);
        body.extend(crc32(data).to_le_bytes());
        body.extend(0u32.to_le_bytes());
        body.extend([29, 0x30]);
        body.extend((name.len() as u16).to_le_bytes());
        body.extend(attributes.to_le_bytes());
        if flags & 0x100 != 0 {
            body.extend([0; 8]);
        }
        body.extend(name);
        if flags & 0x1000 != 0 {
            // mtime with 1 fractional byte and ctime with a DOS time.
            body.extend([0x00, 0x98, 0x11, 0, 0, 0, 0]);
        }
        body
    }

    #[test]
    fn rar5_orders_physical_files_and_ignores_quick_open_cache() {
        let mut bytes = r5_main(0, &[6, 1, 1, 0x80, 0x80, 0x80, 0]);
        let comment = r5_file(b"CMT", 1, 0, 0, 0, 0, 0);
        r5_block(&mut bytes, 3, 4, &comment, &[], b"x");
        let crc = crc32(b"abc");
        let unicode = r5_file("资料/使用说明.txt".as_bytes(), 3, 0x20, 0, 0, 6, crc);
        // FILETIME extra plus mtime in the fixed fields above.
        r5_block(
            &mut bytes,
            2,
            0,
            &unicode,
            &[10, 3, 2, 0, 0, 0, 0, 0, 0, 0, 0],
            b"abc",
        );
        let dir = r5_file("资料".as_bytes(), 0, 0o040755, 0, 1, 5, 0);
        r5_block(&mut bytes, 2, 2, &dir, &[], &[]);
        let empty = r5_file(b"empty", 0, 0x20, 0, 0, 0, 0);
        r5_block(&mut bytes, 2, 2, &empty, &[], &[]);
        let mut cache = Vec::new();
        let bogus = r5_file(b"../not-a-physical-file", 0, 0, 0, 0, 0, 0);
        r5_block(&mut cache, 2, 0, &bogus, &[], &[]);
        let qo = r5_file(b"QO", cache.len() as u64, 0, 0, 0, 0, 0);
        r5_block(&mut bytes, 3, 4, &qo, &[], &cache);
        r5_end(&mut bytes, 0);
        let entries = parse(&bytes).unwrap();
        let actual: Vec<_> = entries
            .iter()
            .map(|(entry, crc)| {
                (
                    entry.path.as_str(),
                    entry.is_dir,
                    entry.size,
                    entry.compressed_size,
                    *crc,
                )
            })
            .collect();
        assert_eq!(
            actual,
            [
                ("资料/使用说明.txt", false, 3, 3, Some(crc)),
                ("资料", true, 0, 0, Some(0)),
                ("empty", false, 0, 0, None),
            ]
        );
    }

    #[test]
    fn rar5_compression_versions_and_fractional_dictionary_limits() {
        let valid = [0, 1 | (9 << 10) | (31 << 15), 1 | (10 << 10) | (1 << 20)];
        let invalid = [2, 1 | (10 << 10) | (1 << 15), 11 << 10, 1 << 14, 1 << 21];
        for (compression, ok) in valid
            .into_iter()
            .map(|n| (n, true))
            .chain(invalid.into_iter().map(|n| (n, false)))
        {
            let mut bytes = r5_main(0, &[]);
            let body = r5_file(b"file", 1, 0x20, compression | (3 << 7), 0, 4, 1);
            r5_block(&mut bytes, 2, 0, &body, &[], b"x");
            r5_end(&mut bytes, 0);
            assert_eq!(parse(&bytes).is_ok(), ok, "compression {compression:#x}");
        }
    }

    #[test]
    fn rar5_rejects_semantic_flags_services_and_extras() {
        let valid = r5_file(b"a", 0, 0x20, 0, 0, 4, 0);
        for (kind, flags, extra) in [
            (4, 0, &[][..]), // encrypted headers
            (2, 8, &[]),
            (2, 16, &[]), // split before/after
            (2, 0x80, &[]),
            (6, 4, &[]),                    // unknown flags/type
            (2, 0, &[1, 1]),                // file crypto
            (2, 0, &[3, 4, 0, 1]),          // renamed historical file version
            (2, 0, &[5, 5, 4, 0, 1, b'x']), // hard link
            (2, 0, &[1, 8]),
            (2, 0, &[0]),
            (2, 0, &[9, 3, 0]), // unknown/malformed extra
            (3, 0, &[]),        // unknown service
        ] {
            let mut bytes = r5_main(0, &[]);
            r5_block(&mut bytes, kind, flags, &valid, extra, &[]);
            r5_end(&mut bytes, 0);
            assert!(
                parse(&bytes).is_err(),
                "kind {kind}, flags {flags}, extra {extra:?}"
            );
        }
        for (name, size, attributes, host, flags) in [
            (&b"a"[..], 0, 0x20, 0, 8), // unknown size
            (b"a", 0, 0o120777, 1, 0),  // Unix symlink
            (b"a", 0, 0o020600, 1, 0),  // character device
            (b"a", 0, 0x400, 0, 0),     // Windows reparse point
            (b"a", 0, 0x10, 0, 0),      // inconsistent file/dir attributes
            (b"a", 0, 0, 2, 0),         // unknown host
            (b"a\0b", 0, 0x20, 0, 0),
            (b"a\\b", 0, 0o100644, 1, 0), // a literal Unix backslash
            (b"../a", 0, 0x20, 0, 0),
            (b"a:\x01", 0, 0x20, 0, 0),
        ] {
            let mut bytes = r5_main(0, &[]);
            let file = r5_file(name, size, attributes, 0, host, flags, 0);
            r5_block(&mut bytes, 2, 0, &file, &[], &[]);
            r5_end(&mut bytes, 0);
            assert!(
                parse(&bytes).is_err(),
                "{name:?}, attr {attributes:#x}, flags {flags}"
            );
        }
        for unsupported in [b"STM", b"ACL"] {
            let mut bytes = r5_main(0, &[]);
            let service = r5_file(unsupported, 0, 0, 0, 0, 0, 0);
            r5_block(&mut bytes, 3, 0, &service, &[], &[]);
            r5_end(&mut bytes, 0);
            assert!(parse(&bytes).is_err());
        }
        for flags in [1, 2, 32] {
            let mut bytes = r5_main(flags, &[]);
            r5_end(&mut bytes, 0);
            assert!(parse(&bytes).is_err());
        }
        let mut bytes = r5_main(0, &[]);
        r5_end(&mut bytes, 1);
        assert!(parse(&bytes).is_err());
    }

    #[test]
    fn rar4_orders_members_and_decodes_both_unicode_name_forms() {
        let mut bytes = RAR4_MAGIC.to_vec();
        r4_block(&mut bytes, 0x73, 0, &[0; 6], &[]);
        let name = b"abcde\0\x4e\x1bM\x2d\x87\x65\0";
        let flags = 0x9300;
        let first = r4_file(name, b"xyz", 3, 0o100644, flags);
        r4_block(&mut bytes, 0x74, flags, &first, b"xyz");
        let second = r4_file("资料\\空文件".as_bytes(), &[], 2, 0x20, 0x8200);
        r4_block(&mut bytes, 0x74, 0x8200, &second, &[]);
        let dir = r4_file("资料".as_bytes(), &[], 2, 0x10, 0x82e0);
        r4_block(&mut bytes, 0x74, 0x82e0, &dir, &[]);
        r4_block(&mut bytes, 0x7b, 0x4000, &[], &[]);
        let entries = parse(&bytes).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|m| m.0.path.as_str())
                .collect::<Vec<_>>(),
            ["M中文de", "资料\\空文件", "资料"]
        );
        assert_eq!(entries[0].1, Some(crc32(b"xyz")));
        assert!(!entries[1].0.is_dir);
        assert!(entries[2].0.is_dir);
        // Corrected ANSI run and malformed runs/sequences cannot silently fall back.
        assert_eq!(
            rar4_name(b"ab\0\x4e\xc0\x80\x01", true).unwrap(),
            "\u{4e62}\u{4e63}"
        );
        for name in [
            &b"a\0\x4e\xc0\x7f"[..],
            b"a\0\x4e\x80x",
            b"a\0\x4e\x00\0",
            b"a\0\x4e\x00",
        ] {
            assert!(rar4_name(name, true).is_err());
        }
    }

    #[test]
    fn rar4_rejects_multi_volume_encryption_links_and_unknown_blocks() {
        for flags in [1, 0x80, 0x100, 0x200] {
            let mut bytes = RAR4_MAGIC.to_vec();
            r4_block(&mut bytes, 0x73, flags, &[0; 6], &[]);
            r4_block(&mut bytes, 0x7b, 0, &[], &[]);
            assert!(parse(&bytes).is_err());
        }
        for (kind, flags, host, attributes) in [
            (0x74, 1, 2, 0x20),
            (0x74, 2, 2, 0x20), // split
            (0x74, 4, 2, 0x20),
            (0x74, 0x400, 2, 0x20), // encrypted / salt
            (0x74, 0, 3, 0o120777),
            (0x74, 0, 2, 0x400), // Unix link / reparse point
            (0x74, 0x800, 2, 0x20),
            (0x7a, 0, 2, 0x20), // version / service
        ] {
            let mut bytes = RAR4_MAGIC.to_vec();
            r4_block(&mut bytes, 0x73, 0, &[0; 6], &[]);
            let file = r4_file(b"a", &[], host, attributes, flags);
            r4_block(&mut bytes, kind, flags | 0x8000, &file, &[]);
            r4_block(&mut bytes, 0x7b, 0, &[], &[]);
            assert!(parse(&bytes).is_err());
        }
    }

    #[test]
    fn framing_is_crc_checked_exact_and_bounded() {
        let mut bytes = r5_main(0, &[]);
        let file = r5_file(b"file", 1, 0x20, 0, 0, 4, 0);
        r5_block(&mut bytes, 2, 0, &file, &[], b"d");
        r5_end(&mut bytes, 0);
        for cut in 0..bytes.len() {
            assert!(parse(&bytes[..cut]).is_err(), "truncated at {cut}");
        }
        let crc_offset = RAR5_MAGIC.len();
        bytes[crc_offset] ^= 1;
        assert!(parse(&bytes).is_err());
        bytes[crc_offset] ^= 1;
        bytes.push(0);
        assert!(parse(&bytes).is_err());
        let mut bytes = RAR4_MAGIC.to_vec();
        r4_block(&mut bytes, 0x73, 0, &[0; 6], &[]);
        r4_block(&mut bytes, 0x7b, 0, &[], &[]);
        bytes[RAR4_MAGIC.len()] ^= 1;
        assert!(parse(&bytes).is_err());

        let mut overlong_header = RAR5_MAGIC.to_vec();
        overlong_header.extend([0, 0, 0, 0, 0x80, 0x80, 0x80, 0]);
        assert!(parse(&overlong_header).is_err());
        let mut overflow_vint = &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 2][..];
        assert!(vint(&mut overflow_vint).is_err());

        let mut bytes = r5_main(0, &[]);
        let file = r5_file(b"file", 1, 0x20, 0, 0, 4, 0);
        r5_block(&mut bytes, 2, 0, &file, &[], b"d");
        r5_end(&mut bytes, 0);
        // First read and validate headers, then exceed the budget on retained names/entries.
        let mut cursor = Cursor::new(&bytes);
        cursor.set_position(8);
        let mut scan = Scan {
            input: &mut cursor,
            length: bytes.len() as u64,
            pos: 8,
            budget: bytes.len(),
            members: Vec::new(),
        };
        assert!(
            matches!(rar5(&mut scan), Err(AppError::UnsafeArchive(message)) if message.contains("超过限制"))
        );
    }
}
