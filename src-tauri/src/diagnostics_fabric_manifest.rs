//! Bounded metadata evidence from an already-open JAR. This never extracts files.
use std::io::{self, Read, Seek, SeekFrom};

use flate2::{Decompress, FlushDecompress, Status};
use serde::Deserialize;

const NAME: &[u8] = b"fabric.mod.json";
const EOCD: &[u8; 4] = b"PK\x05\x06";
const CENTRAL: &[u8; 4] = b"PK\x01\x02";
const LOCAL: &[u8; 4] = b"PK\x03\x04";
const DESCRIPTOR: &[u8; 4] = b"PK\x07\x08";
const EOCD_LEN: usize = 22;
const CENTRAL_LEN: usize = 46;
const LOCAL_LEN: usize = 30;
const MAX_DIRECTORY: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 16_384;
const MAX_MANIFEST: u32 = 64 * 1024;
const MAX_COMPRESSED: u32 = MAX_MANIFEST + 1024;
const MAX_RATIO: u32 = 100;
const HAS_DESCRIPTOR: u16 = 1 << 3;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Manifest {
    Present { id: String, version: String },
    Absent,
    Invalid,
    Limited,
}

pub(super) fn inspect<R: Read + Seek>(mut reader: R, length: u64) -> Manifest {
    match inspect_inner(&mut reader, length) {
        Ok(manifest) | Err(manifest) => manifest,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    schema_version: u8,
    id: String,
    version: String,
}

fn inspect_inner(reader: &mut (impl Read + Seek), length: u64) -> Result<Manifest, Manifest> {
    let Some((entry, bound)) = find_entry(reader, length)? else {
        return Ok(Manifest::Absent);
    };
    let bytes = read_entry(reader, entry, bound)?;
    let json = std::str::from_utf8(&bytes).map_err(|_| Manifest::Invalid)?;
    // Serde also accepts a positional array when deserializing a struct.
    if !json
        .trim_start_matches([' ', '\n', '\r', '\t'])
        .starts_with('{')
    {
        return Err(Manifest::Invalid);
    }
    let Metadata {
        schema_version,
        id,
        version,
    } = serde_json::from_str(json).map_err(|_| Manifest::Invalid)?;
    let valid_id = (2..=64).contains(&id.len())
        && id.as_bytes()[0].is_ascii_lowercase()
        && id
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'));
    let valid_version = (1..=128).contains(&version.len())
        && version.as_bytes()[0].is_ascii_alphanumeric()
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b));
    if schema_version != 1 || !valid_id || !valid_version {
        return Err(Manifest::Invalid);
    }
    Ok(Manifest::Present { id, version })
}

#[derive(Clone, Copy)]
struct Entry {
    offset: u64,
    flags: u16,
    method: u16,
    crc: u32,
    compressed: u32,
    expanded: u32,
}

impl Entry {
    fn body_fields(self) -> [u32; 3] {
        [self.crc, self.compressed, self.expanded]
    }
}

fn read_at(reader: &mut (impl Read + Seek), offset: u64, bytes: &mut [u8]) -> Result<(), Manifest> {
    reader.seek(SeekFrom::Start(offset))?;
    reader.read_exact(bytes)?;
    Ok(())
}

impl From<io::Error> for Manifest {
    fn from(_: io::Error) -> Self {
        Self::Invalid
    }
}

fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn find_entry(
    reader: &mut (impl Read + Seek),
    length: u64,
) -> Result<Option<(Entry, u64)>, Manifest> {
    if length > u64::from(u32::MAX) {
        return Err(Manifest::Limited);
    }
    if length < EOCD_LEN as u64 || reader.seek(SeekFrom::End(0))? != length {
        return Err(Manifest::Invalid);
    }

    // APPNOTE 4.3.16: only a single-disk ZIP32 EOCD whose comment ends at EOF.
    let tail_len = (length as usize).min(EOCD_LEN + usize::from(u16::MAX));
    let tail_start = length - tail_len as u64;
    let mut tail = vec![0; tail_len];
    read_at(reader, tail_start, &mut tail)?;
    let mut candidates = (0..=tail_len - EOCD_LEN).filter(|&at| {
        tail[at..].starts_with(EOCD)
            && at + EOCD_LEN + usize::from(le16(&tail, at + 20)) == tail_len
    });
    let end = candidates.next().ok_or(Manifest::Invalid)?;
    if candidates.next().is_some() {
        return Err(Manifest::Invalid);
    }
    let footer = &tail[end..];
    let count = usize::from(le16(footer, 10));
    let size = le32(footer, 12);
    let offset = le32(footer, 16);
    if le32(footer, 4) != 0
        || usize::from(le16(footer, 8)) != count
        || count == usize::from(u16::MAX)
        || size == u32::MAX
        || offset == u32::MAX
        || u64::from(offset) + u64::from(size) != tail_start + end as u64
        || count * CENTRAL_LEN > size as usize
    {
        return Err(Manifest::Invalid);
    }
    let offset = u64::from(offset);
    if size as usize > MAX_DIRECTORY || count > MAX_ENTRIES {
        return Err(Manifest::Limited);
    }
    if count == 0 {
        return if offset == 0 && size == 0 {
            Ok(None)
        } else {
            Err(Manifest::Invalid)
        };
    }
    let mut magic = [0; 4];
    read_at(reader, 0, &mut magic)?;
    if &magic != LOCAL {
        return Err(Manifest::Invalid);
    }

    // zip 2.6 indexes by decoded name with last-one-wins semantics. Scan the
    // bounded raw directory instead, then pass only the selected entry to zip.
    let mut directory = vec![0; size as usize];
    read_at(reader, offset, &mut directory)?;
    let mut remaining = directory.as_slice();
    let mut other_ranges = Vec::with_capacity(count - 1);
    let mut found = None;
    let mut first = offset;
    for _ in 0..count {
        let header = remaining.get(..CENTRAL_LEN).ok_or(Manifest::Invalid)?;
        if !header.starts_with(CENTRAL) || le16(header, 34) != 0 {
            return Err(Manifest::Invalid);
        }
        let name_len = usize::from(le16(header, 28));
        let extra_len = usize::from(le16(header, 30));
        let comment_len = usize::from(le16(header, 32));
        let name_end = CENTRAL_LEN + name_len;
        let extra_end = name_end + extra_len;
        let Some((record, rest)) = remaining.split_at_checked(extra_end + comment_len) else {
            return Err(Manifest::Invalid);
        };
        remaining = rest;
        let name_matches = &record[CENTRAL_LEN..name_end] == NAME;
        check_extra(&record[name_end..extra_end], name_matches)?;

        let entry = Entry {
            offset: u64::from(le32(header, 42)),
            flags: le16(header, 8),
            method: le16(header, 10),
            crc: le32(header, 16),
            compressed: le32(header, 20),
            expanded: le32(header, 24),
        };
        let minimum_end =
            entry.offset + LOCAL_LEN as u64 + name_len as u64 + u64::from(entry.compressed);
        if name_len == 0
            || entry.compressed == u32::MAX
            || entry.expanded == u32::MAX
            || minimum_end > offset
        {
            return Err(Manifest::Invalid);
        }
        first = first.min(entry.offset);
        if name_matches {
            if found.replace(entry).is_some() || !regular_file(header[5], le32(header, 38)) {
                return Err(Manifest::Invalid);
            }
        } else {
            other_ranges.push((entry.offset, minimum_end));
        }
    }
    if first != 0 || !remaining.is_empty() {
        return Err(Manifest::Invalid);
    }
    let Some(entry) = found else {
        return Ok(None);
    };
    let mut bound = offset;
    for (start, minimum_end) in other_ranges {
        if start > entry.offset {
            bound = bound.min(start);
        } else if minimum_end > entry.offset {
            return Err(Manifest::Invalid);
        }
    }
    Ok(Some((entry, bound)))
}

fn regular_file(host: u8, attributes: u32) -> bool {
    let unix_type = (attributes >> 16) & 0o170000;
    // Reject DOS volume/directory/device/reparse flags and Unix special types.
    attributes & (0x08 | 0x10 | 0x40 | 0x400) == 0
        && matches!(unix_type, 0 | 0o100000)
        && (attributes == 0 || matches!(host, 0 | 3 | 10 | 19))
}

fn check_extra(mut bytes: &[u8], manifest: bool) -> Result<(), Manifest> {
    while !bytes.is_empty() {
        let header = bytes.get(..4).ok_or(Manifest::Invalid)?;
        let tag = le16(header, 0);
        // ZIP64, and for the manifest fields that can change name, format or file type.
        if tag == 0x0001 || (manifest && matches!(tag, 0x000d | 0x7075 | 0x756e | 0x9901)) {
            return Err(Manifest::Invalid);
        }
        bytes = bytes
            .get(4 + usize::from(le16(header, 2))..)
            .ok_or(Manifest::Invalid)?;
    }
    Ok(())
}

fn read_entry(
    reader: &mut (impl Read + Seek),
    entry: Entry,
    bound: u64,
) -> Result<Vec<u8>, Manifest> {
    let allowed_flags = HAS_DESCRIPTOR | (1 << 11) | if entry.method == 8 { 0b110 } else { 0 };
    if !matches!(entry.method, 0 | 8) || entry.flags & !allowed_flags != 0 {
        return Err(Manifest::Invalid);
    }
    if entry.expanded > MAX_MANIFEST
        || entry.compressed > MAX_COMPRESSED
        || entry.expanded > entry.compressed.saturating_mul(MAX_RATIO)
    {
        return Err(Manifest::Limited);
    }
    if entry.method == 0 && entry.expanded != entry.compressed {
        return Err(Manifest::Invalid);
    }

    let extra_start = entry.offset + LOCAL_LEN as u64 + NAME.len() as u64;
    if extra_start + u64::from(entry.compressed) > bound {
        return Err(Manifest::Invalid);
    }
    let mut local = [0; LOCAL_LEN + NAME.len()];
    read_at(reader, entry.offset, &mut local)?;
    if !local.starts_with(LOCAL)
        || le16(&local, 6) != entry.flags
        || le16(&local, 8) != entry.method
        || usize::from(le16(&local, 26)) != NAME.len()
        || &local[LOCAL_LEN..] != NAME
    {
        return Err(Manifest::Invalid);
    }
    let extra_len = usize::from(le16(&local, 28));
    let data_start = extra_start + extra_len as u64;
    let data_end = data_start + u64::from(entry.compressed);
    let has_descriptor = entry.flags & HAS_DESCRIPTOR != 0;
    if data_end + if has_descriptor { 12 } else { 0 } > bound {
        return Err(Manifest::Invalid);
    }
    for (at, expected) in [14, 18, 22].into_iter().zip(entry.body_fields()) {
        let actual = le32(&local, at);
        if actual != expected && !(has_descriptor && actual == 0) {
            return Err(Manifest::Invalid);
        }
        local[at..at + 4].copy_from_slice(&expected.to_le_bytes());
    }

    let mut extra = vec![0; extra_len];
    reader.read_exact(&mut extra)?;
    check_extra(&extra, true)?;
    let mut compressed = vec![0; entry.compressed as usize];
    reader.read_exact(&mut compressed)?;
    if has_descriptor {
        let mut descriptor = [0; 16];
        let len = (bound - data_end).min(16) as usize;
        reader.read_exact(&mut descriptor[..len])?;
        let matches = |at| {
            [
                le32(&descriptor, at),
                le32(&descriptor, at + 4),
                le32(&descriptor, at + 8),
            ] == entry.body_fields()
        };
        if !(matches(0) || (len == 16 && descriptor.starts_with(DESCRIPTOR) && matches(4))) {
            return Err(Manifest::Invalid);
        }
    }

    let expanded = entry.expanded as usize;
    let mut bytes = vec![0; expanded + 1];
    // zip does not expose the deflate end marker or bytes consumed. Verify the
    // exact compressed span: a valid plaintext CRC alone can hide truncation
    // after the plaintext or trailing data inside the claimed compressed size.
    if entry.method == 8 {
        let mut verifier = Decompress::new(false);
        if !matches!(
            verifier.decompress(&compressed, &mut bytes, FlushDecompress::Finish),
            Ok(Status::StreamEnd)
        ) || verifier.total_in() != u64::from(entry.compressed)
            || verifier.total_out() != u64::from(entry.expanded)
        {
            return Err(Manifest::Invalid);
        }
    }

    // The stream reader does not support descriptors. Its private input uses
    // the verified CRC and sizes above and omits extras that zip could reinterpret.
    local[6..8].copy_from_slice(&(entry.flags & !HAS_DESCRIPTOR).to_le_bytes());
    local[28..30].fill(0);
    let mut stream = local.as_slice().chain(compressed.as_slice());
    let mut file = zip::read::read_zipfile_from_stream(&mut stream)
        .map_err(|_| Manifest::Invalid)?
        .ok_or(Manifest::Invalid)?;
    file.read_exact(&mut bytes[..expanded])?;
    // Read through EOF for zip's CRC check, with space for at most one extra byte.
    if file.read(&mut bytes[expanded..])? != 0 {
        return Err(Manifest::Invalid);
    }
    bytes.truncate(expanded);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    use super::*;

    const JSON: &[u8] = br#"{"schemaVersion":1,"id":"demo_mod-2","version":"2.0.1-beta+DEV_4","jars":[{"file":"nested.jar"}]}"#;

    fn jar(entries: &[(&str, &[u8])], method: CompressionMethod) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.set_comment("archive comment").unwrap();
        for (name, content) in entries {
            writer
                .start_file(
                    *name,
                    SimpleFileOptions::default().compression_method(method),
                )
                .unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn manifest_jar(content: &[u8], method: CompressionMethod) -> Vec<u8> {
        jar(&[("fabric.mod.json", content)], method)
    }

    fn inspect_bytes(bytes: &[u8]) -> Manifest {
        inspect(Cursor::new(bytes), bytes.len() as u64)
    }

    fn expected() -> Manifest {
        Manifest::Present {
            id: "demo_mod-2".into(),
            version: "2.0.1-beta+DEV_4".into(),
        }
    }

    fn footer(bytes: &[u8]) -> usize {
        bytes.windows(4).rposition(|w| w == EOCD).unwrap()
    }

    fn central(bytes: &[u8]) -> usize {
        le32(bytes, footer(bytes) + 16) as usize
    }

    fn put16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn reads_only_the_exact_root_entry() {
        let nested = manifest_jar(JSON, CompressionMethod::Stored);
        let other: &[(&str, &[u8])] = &[
            ("nested.jar", &nested),
            ("nested/fabric.mod.json", JSON),
            ("./fabric.mod.json", JSON),
            ("fabric.mod.json/", b""),
            ("FABRIC.MOD.JSON", JSON),
        ];
        assert_eq!(
            inspect_bytes(&jar(other, CompressionMethod::Stored)),
            Manifest::Absent
        );
        assert_eq!(
            inspect_bytes(&jar(&[], CompressionMethod::Stored)),
            Manifest::Absent
        );
        for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
            let mut entries = other.to_vec();
            entries.insert(2, ("fabric.mod.json", JSON));
            assert_eq!(inspect_bytes(&jar(&entries, method)), expected());
        }
    }

    #[test]
    fn requires_one_schema_one_string_identity() {
        for value in [
            r#"[1,"demo_mod","v1"]"#,
            r#"{"schemaVersion":0,"id":"demo_mod","version":"v1"}"#,
            r#"{"schemaVersion":1.0,"id":"demo_mod","version":"v1"}"#,
            r#"{"schemaVersion":"1","id":"demo_mod","version":"v1"}"#,
            r#"{"schemaVersion":1,"schemaVersion":1,"id":"demo_mod","version":"v1"}"#,
            r#"{"id":"demo_mod","version":"v1"}"#,
            r#"{"schemaVersion":1,"id":"demo_mod","id":"other","version":"v1"}"#,
            r#"{"schemaVersion":1,"id":"demo_mod","version":"v1","version":"v2"}"#,
            r#"{"schemaVersion":1,"id":10,"version":"v1"}"#,
            r#"{"schemaVersion":1,"id":"demo_mod","version":10}"#,
            r#"{"schemaVersion":1,"id":"demo_mod","version":"v1"} false"#,
        ] {
            assert_eq!(
                inspect_bytes(&manifest_jar(value.as_bytes(), CompressionMethod::Stored)),
                Manifest::Invalid,
                "{value}"
            );
        }
        for (id, version) in [
            ("m", "v1"),
            ("Mod", "v1"),
            ("1mod", "v1"),
            ("mo.d", "v1"),
            ("m\u{00f6}d", "v1"),
            ("mod", ""),
            ("mod", "+v1"),
            ("mod", "v 1"),
            ("mod", "v/1"),
            ("mod", "v\u{00f6}"),
            (&"m".repeat(65), "1"),
            ("mod", &"1".repeat(129)),
        ] {
            let json =
                serde_json::json!({"schemaVersion": 1, "id": id, "version": version}).to_string();
            assert_eq!(
                inspect_bytes(&manifest_jar(json.as_bytes(), CompressionMethod::Stored)),
                Manifest::Invalid,
                "{json}"
            );
        }
        let id = "m".repeat(64);
        let version = "1".repeat(128);
        let json =
            serde_json::json!({"schemaVersion": 1, "id": id, "version": version}).to_string();
        assert_eq!(
            inspect_bytes(&manifest_jar(json.as_bytes(), CompressionMethod::Stored)),
            Manifest::Present { id, version }
        );
    }

    #[test]
    fn rejects_duplicates_before_the_zip_library_can_collapse_them() {
        let mut bytes = jar(
            &[("fabric.mod.json", JSON), ("gabric.mod.json", JSON)],
            CompressionMethod::Stored,
        );
        let start = central(&bytes);
        let at = bytes[start..]
            .windows(NAME.len())
            .position(|w| w == b"gabric.mod.json")
            .unwrap();
        let second = start + at - CENTRAL_LEN;
        let local = le32(&bytes, second + 42) as usize;
        bytes[start + at] = b'f';
        bytes[local + LOCAL_LEN] = b'f';
        let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        assert_eq!(archive.len(), 1);
        assert_eq!(
            archive.by_name("fabric.mod.json").unwrap().header_start(),
            local as u64
        );
        assert_eq!(inspect_bytes(&bytes), Manifest::Invalid);
    }

    #[test]
    fn rejects_nonregular_encrypted_and_unsupported_entries() {
        let bytes = manifest_jar(JSON, CompressionMethod::Stored);
        let cd = central(&bytes);
        for attributes in [
            (0o120777 << 16),
            (0o010644 << 16),
            (0o040755 << 16),
            0x10,
            0x400,
        ] {
            let mut bad = bytes.clone();
            bad[cd + 5] = 3;
            put32(&mut bad, cd + 38, attributes);
            assert_eq!(inspect_bytes(&bad), Manifest::Invalid);
        }
        for flag in [1, 1 << 6, 1 << 13] {
            let mut bad = bytes.clone();
            put16(&mut bad, cd + 8, flag);
            put16(&mut bad, 6, flag);
            assert_eq!(inspect_bytes(&bad), Manifest::Invalid);
        }
        let mut bad = bytes;
        put16(&mut bad, cd + 10, 12);
        put16(&mut bad, 8, 12);
        assert_eq!(inspect_bytes(&bad), Manifest::Invalid);
    }

    #[test]
    fn rejects_local_mismatch_incorrect_length_and_bad_crc() {
        let bytes = manifest_jar(JSON, CompressionMethod::Deflated);
        let cd = central(&bytes);
        let mut name_mismatch = bytes.clone();
        name_mismatch[LOCAL_LEN] = b'F';
        assert_eq!(inspect_bytes(&name_mismatch), Manifest::Invalid);

        let crc = le32(&bytes, 14) ^ 1;
        for (local, global, value) in [
            (14, 16, crc),
            (22, 24, JSON.len() as u32 - 1),
            (22, 24, JSON.len() as u32 + 1),
        ] {
            let mut bad = bytes.clone();
            put32(&mut bad, local, value);
            put32(&mut bad, cd + global, value);
            assert_eq!(inspect_bytes(&bad), Manifest::Invalid);
        }
    }

    #[test]
    fn checks_signed_and_unsigned_descriptors() {
        let bytes = manifest_jar(JSON, CompressionMethod::Deflated);
        let cd = central(&bytes);
        for signed in [false, true] {
            let mut descriptor = Vec::new();
            if signed {
                descriptor.extend_from_slice(DESCRIPTOR);
            }
            descriptor.extend_from_slice(&bytes[14..26]);
            let mut streaming = bytes.clone();
            streaming.splice(cd..cd, descriptor.iter().copied());
            let new_cd = cd + descriptor.len();
            let end = footer(&streaming);
            put32(&mut streaming, end + 16, new_cd as u32);
            put16(&mut streaming, 6, HAS_DESCRIPTOR);
            put16(&mut streaming, new_cd + 8, HAS_DESCRIPTOR);
            streaming[14..26].fill(0);
            assert_eq!(inspect_bytes(&streaming), expected());
            streaming[new_cd - 1] ^= 1;
            assert_eq!(inspect_bytes(&streaming), Manifest::Invalid);
        }
    }

    #[test]
    fn requires_exact_deflate_span_even_with_valid_plaintext_crc() {
        let bytes = manifest_jar(JSON, CompressionMethod::Deflated);
        let cd = central(&bytes);
        let start = LOCAL_LEN + NAME.len();
        let size = JSON.len() as u16;
        let mut no_end_marker = vec![0]; // non-final stored DEFLATE block
        no_end_marker.extend_from_slice(&size.to_le_bytes());
        no_end_marker.extend_from_slice(&(!size).to_le_bytes());
        no_end_marker.extend_from_slice(JSON);
        let mut trailing = bytes[start..cd].to_vec();
        trailing.push(0);
        for replacement in [no_end_marker, trailing] {
            let mut bad = bytes.clone();
            let len = replacement.len();
            bad.splice(start..cd, replacement);
            let end = footer(&bad);
            put32(&mut bad, end + 16, (start + len) as u32);
            put32(&mut bad, 18, len as u32);
            put32(&mut bad, start + len + 20, len as u32);
            assert_eq!(inspect_bytes(&bad), Manifest::Invalid);
        }
    }

    #[test]
    fn rejects_invalid_directory_layouts() {
        let bytes = manifest_jar(JSON, CompressionMethod::Stored);
        let end = footer(&bytes);
        for (at, value) in [(4, 1), (8, 0), (10, u16::MAX), (20, 0)] {
            let mut bad = bytes.clone();
            put16(&mut bad, end + at, value);
            assert_eq!(inspect_bytes(&bad), Manifest::Invalid);
        }
        let mut bad = bytes;
        let cd = central(&bad);
        put16(&mut bad, cd + 30, u16::MAX);
        assert_eq!(inspect_bytes(&bad), Manifest::Invalid);
        assert_eq!(inspect_bytes(b"not a jar"), Manifest::Invalid);

        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "fabric.mod.json",
                SimpleFileOptions::default().large_file(true),
            )
            .unwrap();
        writer.write_all(JSON).unwrap();
        assert_eq!(
            inspect_bytes(writer.finish().unwrap().get_ref()),
            Manifest::Invalid
        );
    }

    struct Meter<'a> {
        cursor: Cursor<&'a [u8]>,
        reads: usize,
    }

    impl Read for Meter<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let read = self.cursor.read(bytes)?;
            self.reads += read;
            Ok(read)
        }
    }

    impl Seek for Meter<'_> {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.cursor.seek(pos)
        }
    }

    #[test]
    fn bounds_payload_and_directory_work() {
        let mut padded = JSON.to_vec();
        padded.resize(MAX_MANIFEST as usize, b' ');
        assert_eq!(
            inspect_bytes(&manifest_jar(&padded, CompressionMethod::Stored)),
            expected()
        );
        assert_eq!(
            inspect_bytes(&manifest_jar(&padded, CompressionMethod::Deflated)),
            Manifest::Limited
        );
        padded.push(b' ');
        assert_eq!(
            inspect_bytes(&manifest_jar(&padded, CompressionMethod::Stored)),
            Manifest::Limited
        );

        let mut bytes = vec![0; MAX_DIRECTORY + 5 + EOCD_LEN];
        let end = MAX_DIRECTORY + 5;
        bytes[end..end + 4].copy_from_slice(EOCD);
        put16(&mut bytes, end + 8, 1);
        put16(&mut bytes, end + 10, 1);
        put32(&mut bytes, end + 12, (MAX_DIRECTORY + 1) as u32);
        put32(&mut bytes, end + 16, 4);
        let mut meter = Meter {
            cursor: Cursor::new(&bytes),
            reads: 0,
        };
        assert_eq!(inspect(&mut meter, bytes.len() as u64), Manifest::Limited);
        assert_eq!(meter.reads, EOCD_LEN + usize::from(u16::MAX));

        let large = vec![0; 3 * MAX_DIRECTORY];
        let bytes = jar(
            &[("large.dat", &large), ("fabric.mod.json", JSON)],
            CompressionMethod::Stored,
        );
        let mut meter = Meter {
            cursor: Cursor::new(&bytes),
            reads: 0,
        };
        assert_eq!(inspect(&mut meter, bytes.len() as u64), expected());
        assert!(meter.reads < 2 * MAX_DIRECTORY);
    }
}
