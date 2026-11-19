//! Bounded, read-only TES4 header inspection for Bethesda loose plugins.
//!
//! This module deliberately reads only the exact plugin files named by an
//! already-authorized `Plugins.txt`. It does not scan Data, open archives,
//! follow VFS mappings, sort the load order, or interpret records after TES4.
//! It does validate the declared order enough to point out disabled or
//! late-listed masters; the order remains evidence from `Plugins.txt`, not a
//! claim about the effective loader order.

use super::{Context, DiagnosticFile, ReadOptions};
use crate::{error::AppError, plugins::validate_relative_path, safe_fs};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek, SeekFrom},
    path::Path,
};

const RECORD_HEADER_BYTES: usize = 24;
const SUBRECORD_HEADER_BYTES: usize = 6;
const MAX_HEADER_BYTES: usize = 256 * 1024;
const MAX_HEADER_DATA_BYTES: usize = 128 * 1024;
const MAX_TOTAL_HEADER_BYTES: usize = 4 * 1024 * 1024;
const MAX_PLUGINS: usize = 64;
const MAX_MASTERS: usize = 256;
const MAX_SUBRECORDS: usize = 4096;
const MAX_MASTER_NAME_BYTES: usize = 255;
const TES4_COMPRESSED_FLAG: u32 = 0x0004_0000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BethesdaScanStatus {
    Scanned,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginHeaderStatus {
    Parsed,
    Missing,
    Unreadable,
    Invalid,
    Limited,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BethesdaPluginEvidence {
    pub name: String,
    pub path: String,
    pub enabled: bool,
    pub list_line: usize,
    /// One-based position among valid enabled entries in Plugins.txt.
    pub load_order: usize,
    pub header_status: PluginHeaderStatus,
    pub masters: Vec<String>,
    pub missing_masters: Vec<String>,
    pub unlisted_masters: Vec<String>,
    pub disabled_masters: Vec<String>,
    pub masters_after_plugin: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BethesdaEvidence {
    pub status: BethesdaScanStatus,
    pub plugins: Vec<BethesdaPluginEvidence>,
    pub notes: Vec<String>,
}

#[derive(Debug)]
enum HeaderIssue {
    Missing,
    Unreadable(String),
    Invalid(String),
    Limited(String),
}

impl HeaderIssue {
    fn status(&self) -> PluginHeaderStatus {
        match self {
            Self::Missing => PluginHeaderStatus::Missing,
            Self::Unreadable(_) => PluginHeaderStatus::Unreadable,
            Self::Invalid(_) => PluginHeaderStatus::Invalid,
            Self::Limited(_) => PluginHeaderStatus::Limited,
        }
    }

    fn note(&self) -> String {
        match self {
            Self::Missing => "Plugins.txt 声明的插件文件不存在".into(),
            Self::Unreadable(message) => format!("无法安全读取插件头: {message}"),
            Self::Invalid(message) => format!("插件 TES4 头无效: {message}"),
            Self::Limited(message) => format!("插件 TES4 头超出有界解析范围: {message}"),
        }
    }
}

#[derive(Debug)]
struct ParsedHeader {
    masters: Vec<String>,
}

#[derive(Debug, Clone)]
struct ListedPlugin {
    name: String,
    enabled: bool,
    line: usize,
    load_order: usize,
}

/// Inspect valid, enabled entries from the already-read Plugins.txt. The
/// caller remains responsible for the ordinary list-entry checks and evidence.
pub(super) fn inspect(
    ctx: &Context,
    options: &ReadOptions,
    file: &DiagnosticFile,
) -> (BethesdaEvidence, Vec<BethesdaFinding>) {
    let mut findings = Vec::new();
    let entries = listed_plugins(file);
    let mut listed = BTreeMap::new();
    for entry in &entries {
        listed
            .entry(entry.name.to_ascii_lowercase())
            .or_insert_with(|| entry.clone());
    }

    let mut evidence = BethesdaEvidence {
        status: if file.truncated {
            BethesdaScanStatus::Partial
        } else {
            BethesdaScanStatus::Scanned
        },
        plugins: vec![],
        notes: vec![
            "只读取 Plugins.txt 中声明且启用的 Data/*.esp|*.esm|*.esl；不扫描归档、BSA/BA2、MO2/VFS 或其他目录。".into(),
        ],
    };
    if file.truncated {
        evidence
            .notes
            .push("Plugins.txt 读取不完整；仅对已结束的物理行尝试解析。".into());
    }
    if entries.len() > MAX_PLUGINS {
        evidence.status = BethesdaScanStatus::Partial;
        evidence.notes.push(format!(
            "有效插件条目超过 {MAX_PLUGINS} 项；剩余条目未读取插件头。"
        ));
    }

    let data = match ctx.root("data") {
        Ok(path) => path,
        Err(error) => {
            evidence.status = BethesdaScanStatus::Unavailable;
            evidence.notes.push(format!("无法确认 Data 根: {error}"));
            return (evidence, findings);
        }
    };
    let mut budget = options.max_total_bytes.min(MAX_TOTAL_HEADER_BYTES);
    let mut seen_paths = BTreeSet::new();
    let enabled_count = entries.iter().filter(|entry| entry.enabled).count();

    for entry in entries
        .into_iter()
        .filter(|entry| entry.enabled)
        .take(MAX_PLUGINS)
    {
        let key = entry.name.to_ascii_lowercase();
        if !seen_paths.insert(key) {
            continue;
        }
        let path = data.join(&entry.name);
        let path_text = path.display().to_string();
        let mut row = BethesdaPluginEvidence {
            name: entry.name.clone(),
            path: path_text.clone(),
            enabled: entry.enabled,
            list_line: entry.line,
            load_order: entry.load_order,
            header_status: PluginHeaderStatus::Parsed,
            masters: vec![],
            missing_masters: vec![],
            unlisted_masters: vec![],
            disabled_masters: vec![],
            masters_after_plugin: vec![],
            notes: vec![],
        };

        if budget < 1 {
            row.header_status = PluginHeaderStatus::Limited;
            row.notes.push("插件头总读取上限已用尽".into());
            evidence.status = BethesdaScanStatus::Partial;
            evidence.plugins.push(row);
            continue;
        }
        let parsed = read_header(ctx, &path, &mut budget);
        match parsed {
            Ok(header) => {
                row.masters = header.masters;
                for master in &row.masters {
                    let normalized = master.to_ascii_lowercase();
                    match listed.get(&normalized) {
                        None => row.unlisted_masters.push(master.clone()),
                        Some(listed_master) if !listed_master.enabled => {
                            row.disabled_masters.push(master.clone())
                        }
                        Some(listed_master) if listed_master.load_order > entry.load_order => {
                            row.masters_after_plugin.push(master.clone())
                        }
                        Some(_) => {}
                    }
                    let master_path = data.join(master);
                    match ctx
                        .guard(&master_path)
                        .and_then(|_| safe_fs::open_regular(&master_path))
                        .and_then(|file| safe_fs::verify_regular_at(&file, &master_path))
                    {
                        Ok(()) => {}
                        Err(AppError::Io(error))
                            if error.kind() == std::io::ErrorKind::NotFound =>
                        {
                            row.missing_masters.push(master.clone())
                        }
                        Err(error) => row.notes.push(format!(
                            "无法核对主文件 {master}: {}",
                            bounded_note(&error.to_string())
                        )),
                    }
                }
                if !row.missing_masters.is_empty() {
                    findings.push(BethesdaFinding::MissingMaster {
                        list_file: file.file.clone(),
                        file: path_text.clone(),
                        line: entry.line,
                        plugin: entry.name.clone(),
                        masters: row.missing_masters.clone(),
                    });
                }
                if !row.unlisted_masters.is_empty() {
                    row.notes.push(
                        "主文件未出现在当前 Plugins.txt；可能由其他配置、VFS 或游戏内置规则提供，未判定为故障。"
                            .into(),
                    );
                    evidence.status = BethesdaScanStatus::Partial;
                }
                if !row.disabled_masters.is_empty() {
                    row.notes.push(format!(
                        "主文件已列出但未启用: {}",
                        row.disabled_masters.join(", ")
                    ));
                    evidence.status = BethesdaScanStatus::Partial;
                    findings.push(BethesdaFinding::DisabledMaster {
                        list_file: file.file.clone(),
                        file: path_text.clone(),
                        line: entry.line,
                        plugin: entry.name.clone(),
                        masters: row.disabled_masters.clone(),
                    });
                }
                if !row.masters_after_plugin.is_empty() {
                    row.notes.push(format!(
                        "主文件在当前 Plugins.txt 中排在插件之后: {}",
                        row.masters_after_plugin.join(", ")
                    ));
                    evidence.status = BethesdaScanStatus::Partial;
                    findings.push(BethesdaFinding::MasterAfterPlugin {
                        list_file: file.file.clone(),
                        file: path_text.clone(),
                        line: entry.line,
                        plugin: entry.name.clone(),
                        masters: row.masters_after_plugin.clone(),
                    });
                }
            }
            Err(issue) => {
                row.header_status = issue.status();
                row.notes.push(issue.note());
                evidence.status = BethesdaScanStatus::Partial;
                evidence.notes.push(format!(
                    "{}: {}",
                    entry.name,
                    bounded_note(&row.notes.last().cloned().unwrap_or_default())
                ));
            }
        }
        evidence.plugins.push(row);
    }
    if evidence.plugins.len() < enabled_count.min(MAX_PLUGINS) {
        evidence.status = BethesdaScanStatus::Partial;
    }
    (evidence, findings)
}

#[derive(Debug)]
pub(super) enum BethesdaFinding {
    MissingMaster {
        list_file: String,
        file: String,
        line: usize,
        plugin: String,
        masters: Vec<String>,
    },
    DisabledMaster {
        list_file: String,
        file: String,
        line: usize,
        plugin: String,
        masters: Vec<String>,
    },
    MasterAfterPlugin {
        list_file: String,
        file: String,
        line: usize,
        plugin: String,
        masters: Vec<String>,
    },
}

impl BethesdaFinding {
    pub(super) fn add_to(&self, diagnosis: &mut super::Diagnosis, options: &ReadOptions) {
        match self {
            Self::MissingMaster {
                list_file,
                file,
                line,
                plugin,
                masters,
            } => diagnosis.add(
                options,
                super::CauseKind::Dependency,
                super::Basis::Observed,
                list_file,
                Some(*line),
                &format!(
                    "{plugin} ({file}) 的 TES4 MAST 主文件不存在: {}",
                    masters.join(", ")
                ),
                "bethesda_plugin_header",
                "插件头声明的主文件在当前 Data 布局中缺失；未验证 MO2/VFS 或完整游戏加载状态",
            ),
            Self::DisabledMaster {
                list_file,
                file,
                line,
                plugin,
                masters,
            } => diagnosis.add(
                options,
                super::CauseKind::Dependency,
                super::Basis::Observed,
                list_file,
                Some(*line),
                &format!(
                    "{plugin} ({file}) 的 TES4 MAST 主文件已列出但未启用: {}",
                    masters.join(", ")
                ),
                "bethesda_plugin_order",
                "主文件存在且被 Plugins.txt 声明，但当前条目未启用；未验证 MO2/VFS 或完整游戏加载状态",
            ),
            Self::MasterAfterPlugin {
                list_file,
                file,
                line,
                plugin,
                masters,
            } => diagnosis.add(
                options,
                super::CauseKind::LoadOrder,
                super::Basis::Observed,
                list_file,
                Some(*line),
                &format!(
                    "{plugin} ({file}) 的 TES4 MAST 主文件排在插件之后: {}",
                    masters.join(", ")
                ),
                "bethesda_plugin_order",
                "仅核对 Plugins.txt 声明顺序；未排序、未验证 LOOT、ESL 槽位、MO2/VFS 或游戏实际加载状态",
            ),
        }
    }
}

fn listed_plugins(file: &DiagnosticFile) -> Vec<ListedPlugin> {
    let mut load_order = 0;
    file.lines
        .iter()
        .filter(|line| !line.truncated)
        .filter_map(|line| {
            let text = line.text.trim();
            if text.is_empty() || text.starts_with('#') {
                return None;
            }
            let enabled = text.starts_with('*');
            let name = text.strip_prefix('*').unwrap_or(text).trim();
            let ext = Path::new(name)
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if validate_relative_path(name, false).is_err()
                || name.contains('/')
                || !["esp", "esm", "esl"].contains(&ext.as_str())
            {
                return None;
            }
            if enabled {
                load_order += 1;
            }
            Some(ListedPlugin {
                name: name.to_string(),
                enabled,
                line: line.number,
                load_order: if enabled { load_order } else { 0 },
            })
        })
        .collect()
}

fn read_header(
    ctx: &Context,
    path: &Path,
    budget: &mut usize,
) -> Result<ParsedHeader, HeaderIssue> {
    ctx.guard(path)
        .map_err(|error| HeaderIssue::Unreadable(error.to_string()))?;
    let mut file = safe_fs::open_regular(path).map_err(|error| {
        if matches!(&error, AppError::Io(io) if io.kind() == std::io::ErrorKind::NotFound) {
            HeaderIssue::Missing
        } else {
            HeaderIssue::Unreadable(error.to_string())
        }
    })?;
    let before = file
        .metadata()
        .map_err(|error| HeaderIssue::Unreadable(error.to_string()))?;
    let available = before.len().min(MAX_HEADER_BYTES as u64) as usize;
    let amount = available.min(*budget);
    if amount == 0 {
        return Err(HeaderIssue::Limited("读取预算为零".into()));
    }
    let mut bytes = vec![0u8; amount];
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.read_exact(&mut bytes))
        .map_err(|error| HeaderIssue::Unreadable(error.to_string()))?;
    *budget -= amount;
    safe_fs::verify_regular_at(&file, path)
        .map_err(|error| HeaderIssue::Unreadable(error.to_string()))?;
    let after = file
        .metadata()
        .map_err(|error| HeaderIssue::Unreadable(error.to_string()))?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(HeaderIssue::Unreadable("读取期间文件发生变化".into()));
    }
    parse_header(&bytes, before.len())
}

fn parse_header(bytes: &[u8], file_size: u64) -> Result<ParsedHeader, HeaderIssue> {
    if bytes.len() < RECORD_HEADER_BYTES {
        return Err(HeaderIssue::Invalid("文件短于 TES4 记录头".into()));
    }
    if &bytes[..4] != b"TES4" {
        return Err(HeaderIssue::Invalid("首个记录不是 TES4".into()));
    }
    let data_size = read_u32(bytes, 4)? as usize;
    let flags = read_u32(bytes, 8)?;
    if flags & TES4_COMPRESSED_FLAG != 0 {
        return Err(HeaderIssue::Limited(
            "TES4 记录带压缩标记，未解压解析".into(),
        ));
    }
    if data_size > MAX_HEADER_DATA_BYTES {
        return Err(HeaderIssue::Limited(format!(
            "TES4 数据区 {data_size} 字节超过 {MAX_HEADER_DATA_BYTES} 字节"
        )));
    }
    let end = RECORD_HEADER_BYTES
        .checked_add(data_size)
        .ok_or_else(|| HeaderIssue::Invalid("TES4 数据区大小溢出".into()))?;
    if end > bytes.len() {
        return Err(if file_size > bytes.len() as u64 {
            HeaderIssue::Limited("TES4 数据区超过读取前缀".into())
        } else {
            HeaderIssue::Invalid("TES4 数据区被截断".into())
        });
    }
    parse_subrecords(&bytes[RECORD_HEADER_BYTES..end])
}

fn parse_subrecords(data: &[u8]) -> Result<ParsedHeader, HeaderIssue> {
    let mut offset = 0usize;
    let mut extended_size = None;
    let mut masters = Vec::new();
    let mut seen = BTreeSet::new();
    let mut records = 0usize;
    while offset < data.len() {
        records += 1;
        if records > MAX_SUBRECORDS {
            return Err(HeaderIssue::Limited(format!(
                "TES4 子记录超过 {MAX_SUBRECORDS} 项"
            )));
        }
        if data.len() - offset < SUBRECORD_HEADER_BYTES {
            return Err(HeaderIssue::Invalid("TES4 子记录头被截断".into()));
        }
        let code = &data[offset..offset + 4];
        let declared = u16::from_le_bytes([data[offset + 4], data[offset + 5]]) as usize;
        offset += SUBRECORD_HEADER_BYTES;
        if code == b"XXXX" {
            if declared != 4 || data.len() - offset < 4 {
                return Err(HeaderIssue::Invalid("XXXX 子记录长度无效".into()));
            }
            extended_size = Some(u32::from_le_bytes(
                data[offset..offset + 4].try_into().expect("bounded slice"),
            ) as usize);
            offset += 4;
            continue;
        }
        let size = extended_size.take().unwrap_or(declared);
        let end = offset
            .checked_add(size)
            .ok_or_else(|| HeaderIssue::Invalid("TES4 子记录大小溢出".into()))?;
        if end > data.len() {
            return Err(HeaderIssue::Invalid("TES4 子记录数据被截断".into()));
        }
        if code == b"MAST" {
            if masters.len() >= MAX_MASTERS {
                return Err(HeaderIssue::Limited(format!(
                    "MAST 数量超过 {MAX_MASTERS} 项"
                )));
            }
            let master = parse_master(&data[offset..end])?;
            if !seen.insert(master.to_ascii_lowercase()) {
                return Err(HeaderIssue::Invalid("MAST 主文件重复".into()));
            }
            masters.push(master);
        }
        offset = end;
    }
    if extended_size.is_some() {
        return Err(HeaderIssue::Invalid("XXXX 后缺少目标子记录".into()));
    }
    Ok(ParsedHeader { masters })
}

fn parse_master(bytes: &[u8]) -> Result<String, HeaderIssue> {
    if bytes.is_empty() || bytes.len() > MAX_MASTER_NAME_BYTES + 1 {
        return Err(HeaderIssue::Invalid("MAST 文件名长度无效".into()));
    }
    let nul = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| HeaderIssue::Invalid("MAST 缺少结尾 NUL".into()))?;
    if nul == 0 || bytes[nul + 1..].iter().any(|byte| *byte != 0) {
        return Err(HeaderIssue::Invalid("MAST NUL 后仍有非零数据".into()));
    }
    let text = match String::from_utf8(bytes[..nul].to_vec()) {
        Ok(text) => text,
        Err(_) => {
            let (decoded, _, had_errors) = encoding_rs::WINDOWS_1252.decode(&bytes[..nul]);
            if had_errors || decoded.contains('\u{fffd}') {
                return Err(HeaderIssue::Invalid("MAST 文件名编码无效".into()));
            }
            decoded.into_owned()
        }
    };
    let ext = Path::new(&text)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if validate_relative_path(&text, false).is_err()
        || text.contains(['/', '\\'])
        || !["esp", "esm", "esl"].contains(&ext.as_str())
    {
        return Err(HeaderIssue::Invalid("MAST 不是安全的插件文件名".into()));
    }
    Ok(text)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, HeaderIssue> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| HeaderIssue::Invalid("TES4 记录头字段被截断".into()))?;
    Ok(u32::from_le_bytes(slice.try_into().expect("bounded slice")))
}

fn bounded_note(text: &str) -> String {
    let mut chars = text.chars();
    let note: String = chars.by_ref().take(512).collect();
    if chars.next().is_some() {
        format!("{note}…")
    } else {
        note
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subrecord(code: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(code);
        out.extend_from_slice(&(data.len() as u16).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    fn plugin(masters: &[&str]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend(subrecord(b"HEDR", &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        for master in masters {
            let mut value = master.as_bytes().to_vec();
            value.push(0);
            data.extend(subrecord(b"MAST", &value));
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"TES4");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend(data);
        out
    }

    #[test]
    fn parses_masters_and_rejects_duplicate_master() {
        let parsed = parse_header(&plugin(&["Skyrim.esm", "Update.esm"]), 1024).unwrap();
        assert_eq!(parsed.masters, ["Skyrim.esm", "Update.esm"]);
        assert!(matches!(
            parse_header(&plugin(&["Skyrim.esm", "skyrim.esm"]), 1024),
            Err(HeaderIssue::Invalid(message)) if message.contains("重复")
        ));
    }

    #[test]
    fn rejects_truncated_malformed_and_unsafe_headers() {
        assert!(parse_header(b"TES4", 4).is_err());
        let mut invalid = plugin(&[]);
        invalid[..4].copy_from_slice(b"NOPE");
        assert!(matches!(
            parse_header(&invalid, invalid.len() as u64),
            Err(HeaderIssue::Invalid(message)) if message.contains("TES4")
        ));
        assert!(matches!(
            parse_header(&plugin(&["../outside.esp"]), 1024),
            Err(HeaderIssue::Invalid(_))
        ));
    }

    #[test]
    fn compressed_and_overlarge_headers_are_limited_not_dependencies() {
        let mut compressed = plugin(&[]);
        compressed[8..12].copy_from_slice(&TES4_COMPRESSED_FLAG.to_le_bytes());
        assert!(matches!(
            parse_header(&compressed, compressed.len() as u64),
            Err(HeaderIssue::Limited(_))
        ));
        let mut huge = plugin(&[]);
        huge[4..8].copy_from_slice(&((MAX_HEADER_DATA_BYTES + 1) as u32).to_le_bytes());
        assert!(matches!(
            parse_header(&huge, huge.len() as u64),
            Err(HeaderIssue::Limited(_))
        ));
    }
}
