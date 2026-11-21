//! Evidence from a bounded, untrusted startup log, not runtime activation.
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::{DiagnosticFile, FileKind, ReadStatus};

#[path = "diagnostics_fabric_failure.rs"]
mod failure;
pub(super) use failure::dependency_details;

const MAX_MODS: usize = 100;
const MAX_DECLARED: usize = 4096;
const MAX_VERSION: usize = 128;
const MAX_TREE_INDENT: usize = 256;
const STARTUP: &str = "Loading Minecraft";
const LOADER: &str = "with Fabric Loader";
const FAILURE: &str = "Mod resolution failed";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FabricStatus {
    Unavailable,
    Unrecognized,
    StartupObserved,
    /// The historical selection record was complete; output may be capped.
    SelectionReported,
    PartialSelection,
    ResolutionFailed,
    Ambiguous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FabricEvidence {
    pub(super) status: FabricStatus,
    #[serde(serialize_with = "crate::paths::serialize_user_path")]
    pub(super) file: String,
    pub(super) modified_at: Option<String>,
    pub(super) game_version: Option<String>,
    pub(super) loader_version: Option<String>,
    pub(super) startup_line: Option<usize>,
    pub(super) selection_line: Option<usize>,
    pub(super) failure_line: Option<usize>,
    pub(super) declared_count: Option<usize>,
    pub(super) mods: Vec<FabricMod>,
    /// Valid rows observed but not emitted, not rows missing from the prefix.
    pub(super) omitted_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FabricMod {
    pub(super) id: String,
    pub(super) version: String,
    pub(super) line: usize,
    pub(super) nested: bool,
}

impl FabricEvidence {
    fn empty(file: &DiagnosticFile, status: FabricStatus) -> Self {
        Self {
            status,
            file: "logs/latest.log".into(),
            modified_at: file.modified_at.clone(),
            game_version: None,
            loader_version: None,
            startup_line: None,
            selection_line: None,
            failure_line: None,
            declared_count: None,
            mods: Vec::new(),
            omitted_count: 0,
        }
    }
}

pub(super) fn inspect(file: &DiagnosticFile) -> FabricEvidence {
    if file.status != ReadStatus::Read || file.kind != FileKind::Log {
        return FabricEvidence::empty(file, FabricStatus::Unavailable);
    }

    let mut evidence = FabricEvidence::empty(file, FabricStatus::Unrecognized);
    let mut saw_startup = false;
    let mut header = None;
    for (index, line) in file.lines.iter().enumerate() {
        let Some(record) = Record::parse(&line.text) else {
            continue;
        };
        if record.is("INFO", "FabricLoader/GameProvider")
            && startup_marker(record.message, line.truncated)
        {
            if saw_startup {
                return FabricEvidence::empty(file, FabricStatus::Ambiguous);
            }
            saw_startup = true;
            if !line.truncated {
                if let Some((game, loader)) = startup_versions(record.message) {
                    evidence.status = FabricStatus::StartupObserved;
                    evidence.startup_line = Some(line.number);
                    evidence.game_version = Some(game.into());
                    evidence.loader_version = Some(loader.into());
                }
            }
        }
        if evidence.startup_line.is_none() {
            continue;
        }

        let outcome = if record.is("INFO", "FabricLoader")
            && selection_marker(record.message, line.truncated)
        {
            let count = (!line.truncated)
                .then(|| selection_count(record.message))
                .flatten();
            Header::Selection(count)
        } else if record.is("WARN", "FabricLoader/Resolution")
            && (record.message == FAILURE
                || (line.truncated
                    && record.message.starts_with("Mod resolution")
                    && FAILURE.starts_with(record.message)))
        {
            Header::Failure
        } else {
            continue;
        };
        if header.replace((index, outcome)).is_some() {
            return FabricEvidence::empty(file, FabricStatus::Ambiguous);
        }
    }

    if let Some((index, outcome)) = header {
        let line = &file.lines[index];
        match outcome {
            Header::Failure if !line.truncated => {
                evidence.status = FabricStatus::ResolutionFailed;
                evidence.failure_line = Some(line.number);
            }
            Header::Failure => {}
            Header::Selection(_) if line.truncated => {}
            Header::Selection(count) => {
                evidence.status = FabricStatus::PartialSelection;
                evidence.selection_line = Some(line.number);
                evidence.declared_count = count;
                if let Some(count) = count {
                    inspect_selection(file, index, count, &mut evidence);
                }
            }
        }
    }
    evidence
}

enum Header {
    Selection(Option<usize>),
    Failure,
}

struct Record<'a> {
    thread: &'a str,
    level: &'a str,
    category: Option<&'a str>,
    message: &'a str,
}

impl<'a> Record<'a> {
    fn parse(text: &'a str) -> Option<Self> {
        let (timestamp, text) = text.strip_prefix('[')?.split_once("] [")?;
        let [h1, h2, b':', m1, m2, b':', s1, s2] = *timestamp.as_bytes() else {
            return None;
        };
        if !(h1.is_ascii_digit()
            && h2.is_ascii_digit()
            && (h1 - b'0') * 10 + (h2 - b'0') < 24
            && matches!(m1, b'0'..=b'5')
            && m2.is_ascii_digit()
            && matches!(s1, b'0'..=b'5')
            && s2.is_ascii_digit())
        {
            return None;
        }

        let (context, rest) = text.split_once(']')?;
        let (thread, level) = context.rsplit_once('/')?;
        // Vanilla latest.log omits the category; Fabric Loom's layout names it.
        let (category, message) = if let Some(message) = rest.strip_prefix(": ") {
            (None, message)
        } else {
            let (category, message) = rest.strip_prefix(" (")?.split_once(") ")?;
            (Some(category), message)
        };
        if thread.is_empty()
            || thread.len() > 64
            || !thread
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b" ._#-".contains(&b))
            || !matches!(
                level,
                "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR" | "FATAL"
            )
            || category.is_some_and(|name| {
                name.is_empty()
                    || name.len() > 128
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._/-$".contains(&b))
            })
        {
            return None;
        }
        Some(Self {
            thread,
            level,
            category,
            message,
        })
    }

    fn is(&self, level: &str, category: &str) -> bool {
        self.thread == "main" && self.level == level && self.category.is_none_or(|c| c == category)
    }
}

fn startup_marker(message: &str, truncated: bool) -> bool {
    let Some(body) = message.strip_prefix(STARTUP) else {
        return truncated && message.starts_with("Loading Mine") && STARTUP.starts_with(message);
    };
    if body.is_empty() {
        return truncated;
    }
    let Some(body) = body.strip_prefix(' ') else {
        return false;
    };
    if body.contains(" with Fabric Loader") || body.starts_with(LOADER) {
        return true;
    }
    match body.split_once(' ') {
        Some((_, suffix)) => {
            suffix.starts_with("with Fabric") || (truncated && LOADER.starts_with(suffix))
        }
        None => truncated,
    }
}

fn startup_versions(message: &str) -> Option<(&str, &str)> {
    let (game, loader) = message
        .strip_prefix("Loading Minecraft ")?
        .split_once(" with Fabric Loader ")?;
    (valid_version(game) && valid_version(loader)).then_some((game, loader))
}

fn selection_marker(message: &str, truncated: bool) -> bool {
    let Some(body) = message.strip_prefix("Loading ") else {
        return false;
    };
    let body = body.trim_start_matches(' ');
    let Some((count, label)) = body.split_once(' ') else {
        return truncated && !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit());
    };
    let label = label.trim_start_matches(' ');
    label.starts_with("mod:")
        || label.starts_with("mods:")
        || ((truncated || matches!(label, "mod" | "mods"))
            && !count.is_empty()
            && count.bytes().all(|b| b.is_ascii_digit())
            && ("mod:".starts_with(label) || "mods:".starts_with(label)))
}

fn selection_count(message: &str) -> Option<usize> {
    let (count, label) = message.strip_prefix("Loading ")?.split_once(' ')?;
    if !matches!(label, "mod:" | "mods:")
        || count.is_empty()
        || !count.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    count.parse().ok().filter(|&count| count <= MAX_DECLARED)
}

fn inspect_selection(
    file: &DiagnosticFile,
    header_index: usize,
    count: usize,
    evidence: &mut FabricEvidence,
) {
    let mut previous = file.lines[header_index].number;
    let mut next = file.lines[header_index + 1..].iter();
    let mut ids = HashSet::new();
    let mut repeated_id = false;
    for seen in 0..count {
        let Some(line) = next.next() else { return };
        if line.truncated || previous.checked_add(1) != Some(line.number) {
            return;
        }
        let Some((id, version, nested)) = mod_record(&line.text) else {
            return;
        };
        if nested && seen == 0 {
            return;
        }
        repeated_id |= !ids.insert(id);
        if seen < MAX_MODS {
            evidence.mods.push(FabricMod {
                id: id.into(),
                version: version.into(),
                line: line.number,
                nested,
            });
        } else {
            evidence.omitted_count += 1;
        }
        previous = line.number;
    }

    // The count alone cannot prove that an adjacent malformed or clipped row
    // is outside this multiline record. EOF in a clipped prefix is not a boundary.
    let complete = match next.next() {
        Some(line) => {
            !line.truncated
                && previous.checked_add(1) == Some(line.number)
                && (line.text.is_empty() || Record::parse(&line.text).is_some())
        }
        None => !file.truncated,
    };
    // Keep duplicate rows in their original order, but their count cannot
    // establish that the distinct selection was fully reported.
    if complete && !repeated_id {
        evidence.status = FabricStatus::SelectionReported;
    }
}

fn mod_record(text: &str) -> Option<(&str, &str, bool)> {
    let mut body = text.trim_start_matches([' ', '\t']);
    let indent = text.len() - body.len();
    if indent == 0 || text.len() > MAX_TREE_INDENT + 4 + 64 + 1 + MAX_VERSION {
        return None;
    }
    let (pair, nested) = if let Some(pair) = body.strip_prefix("- ") {
        (pair, false)
    } else {
        // Upstream uses a tab followed by spaces and optional ancestor pipes.
        while let Some(rest) = body.strip_prefix("| ") {
            body = rest.trim_start_matches(' ');
        }
        let pair = body
            .strip_prefix("|-- ")
            .or_else(|| body.strip_prefix("\\-- "))?;
        (pair, true)
    };
    let (id, version) = pair.split_once(' ')?;
    ((text.len() - pair.len()) <= MAX_TREE_INDENT + 4 && valid_id(id) && valid_version(version))
        .then_some((id, version, nested))
}

fn valid_id(id: &str) -> bool {
    (2..=64).contains(&id.len())
        && id.as_bytes()[0].is_ascii_lowercase()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}

fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= MAX_VERSION
        && version.as_bytes()[0].is_ascii_alphanumeric()
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::LogLine;

    const START: &str =
        "[12:34:56] [main/INFO]: Loading Minecraft 1.21.1 with Fabric Loader 0.16.14";
    const LOOM_START: &str =
        "[12:34:56] [main/INFO] (FabricLoader/GameProvider) Loading Minecraft 24w40a with Fabric Loader 0.16.14";
    const ONE: &str = "[12:34:57] [main/INFO]: Loading 1 mod:";
    const LOOM_ONE: &str = "[12:34:57] [main/INFO] (FabricLoader) Loading 1 mod:";
    const FAILED: &str = "[12:34:57] [main/WARN]: Mod resolution failed";
    const LOOM_FAILED: &str =
        "[12:34:57] [main/WARN] (FabricLoader/Resolution) Mod resolution failed";

    fn log(lines: &[&str]) -> DiagnosticFile {
        DiagnosticFile {
            file: "/private/instance/logs/latest.log".into(),
            kind: FileKind::Log,
            status: ReadStatus::Read,
            size: None,
            modified_at: Some("2026-11-18T00:00:00Z".into()),
            bytes_read: 0,
            truncated: false,
            encoding: None,
            lines: lines
                .iter()
                .enumerate()
                .map(|(index, &text)| LogLine {
                    number: index + 40,
                    text: text.into(),
                    truncated: false,
                })
                .collect(),
            notes: Vec::new(),
            bg3: None,
        }
    }

    #[test]
    fn preserves_upstream_tree_order_and_physical_lines() {
        let mut file = log(&[
            START,
            "[12:34:57] [main/INFO]: Loading 5 mods:",
            "\t- fabric-api 0.100.0+1.21",
            "\t   |-- api_base 1.2.0",
            "\t   |    \\-- inner 2.0-rc.1",
            "\t   \\-- api_tools 3.0",
            "\t- minecraft 1.21.1",
            "[12:34:58] [Render thread/INFO]: New log record",
            "later clipped text",
        ]);
        file.truncated = true;
        file.lines[8].truncated = true;

        let evidence = inspect(&file);
        assert_eq!(evidence.status, FabricStatus::SelectionReported);
        assert_eq!(evidence.file, "logs/latest.log");
        assert_eq!(evidence.modified_at, file.modified_at);
        assert_eq!(evidence.startup_line, Some(40));
        assert_eq!(evidence.selection_line, Some(41));
        assert_eq!(evidence.declared_count, Some(5));
        assert_eq!(
            evidence
                .mods
                .iter()
                .map(|m| (m.id.as_str(), m.version.as_str(), m.line, m.nested))
                .collect::<Vec<_>>(),
            [
                ("fabric-api", "0.100.0+1.21", 42, false),
                ("api_base", "1.2.0", 43, true),
                ("inner", "2.0-rc.1", 44, true),
                ("api_tools", "3.0", 45, true),
                ("minecraft", "1.21.1", 46, false),
            ]
        );
        let json = serde_json::to_value(evidence).unwrap();
        assert_eq!(json["status"], "selection_reported");
        assert_eq!(json["gameVersion"], "1.21.1");
        assert_eq!(json["loaderVersion"], "0.16.14");
        assert_eq!(json["omittedCount"], 0);
        let decoded: FabricEvidence = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.mods[2].line, 44);
    }

    #[test]
    fn needs_an_exact_startup_and_supported_resolution_warning() {
        let mut file = log(&[START, FAILED]);
        file.status = ReadStatus::Denied;
        assert_eq!(inspect(&file).status, FabricStatus::Unavailable);

        for lines in [
            vec![ONE, "\t- fake 1.0", FAILED],
            vec!["Loading Minecraft 1.21.1 with Fabric Loader 0.16.14", FAILED],
            vec!["[24:34:56] [main/INFO]: Loading Minecraft 1.21.1 with Fabric Loader 0.16.14", FAILED],
            vec!["[12:34:56] [main/INFO]: quoting Loading Minecraft 1.21.1 with Fabric Loader 0.16.14", FAILED],
        ] {
            let evidence = inspect(&log(&lines));
            assert_eq!(evidence.status, FabricStatus::Unrecognized);
            assert_eq!(evidence.failure_line, None);
        }

        let evidence = inspect(&log(&[
            FAILED,
            START,
            "Mod resolution failed",
            "\tat example.Mod resolution failed",
            "[12:34:57] [main/WARN]: java.lang.Exception: Mod resolution failed",
            "[12:34:57] [main/WARN]: Mod resolution failed: not the exact record",
            "[12:34:57] [main/INFO]: Mod resolution failed",
            "[12:34:57] [Render thread/WARN]: Mod resolution failed",
            "[12:34:57] [main/WARN] (Other) Mod resolution failed",
        ]));
        assert_eq!(evidence.status, FabricStatus::StartupObserved);
        assert_eq!(evidence.failure_line, None);

        for (startup, failure) in [(START, FAILED), (LOOM_START, LOOM_FAILED)] {
            let evidence = inspect(&log(&[startup, failure]));
            assert_eq!(evidence.status, FabricStatus::ResolutionFailed);
            assert_eq!(evidence.failure_line, Some(41));
            assert!(evidence.mods.is_empty());
        }
    }

    #[test]
    fn rejects_mixed_sessions_and_duplicate_outcomes() {
        let malformed = "[12:34:57] [main/INFO]: Loading Minecraft ?? with Fabric Loader";
        for lines in [
            vec![START, ONE, "\t- fabricapi 1.0", LOOM_START],
            vec![malformed, START],
            vec![START, malformed],
            vec![START, ONE, "\t- fabricapi 1.0", LOOM_ONE],
            vec![LOOM_START, LOOM_FAILED, FAILED],
            vec![START, LOOM_ONE, "\t- fabricapi 1.0", FAILED],
            vec![START, FAILED, LOOM_ONE, "\t- fabricapi 1.0"],
        ] {
            let evidence = inspect(&log(&lines));
            assert_eq!(evidence.status, FabricStatus::Ambiguous);
            assert!(evidence.mods.is_empty());
            assert_eq!(evidence.game_version, None);
            assert_eq!(evidence.loader_version, None);
            assert_eq!(evidence.startup_line, None);
            assert_eq!(evidence.selection_line, None);
            assert_eq!(evidence.failure_line, None);
        }
    }

    #[test]
    fn clipped_markers_are_not_positive_evidence() {
        let mut file = log(&[START, LOOM_ONE, "\t- fabricapi 1.0"]);
        file.lines[1].truncated = true;
        let evidence = inspect(&file);
        assert_eq!(evidence.status, FabricStatus::StartupObserved);
        assert_eq!(evidence.selection_line, None);
        assert!(evidence.mods.is_empty());

        file.lines[1].text = FAILED.into();
        let evidence = inspect(&file);
        assert_eq!(evidence.status, FabricStatus::StartupObserved);
        assert_eq!(evidence.failure_line, None);

        file.lines[1].text =
            "[12:34:56] [main/INFO] (FabricLoader/GameProvider) Loading Minecraft 1.21 with Fa"
                .into();
        assert_eq!(inspect(&file).status, FabricStatus::Ambiguous);

        file.lines[1].text =
            "[12:34:56] [main/INFO] (FabricLoader/GameProvider) Loading Mine".into();
        assert_eq!(inspect(&file).status, FabricStatus::Ambiguous);

        file.lines.remove(0);
        assert_eq!(inspect(&file).status, FabricStatus::Unrecognized);
    }

    #[test]
    fn incomplete_sections_keep_only_contiguous_valid_rows() {
        const TWO: &str = "[12:34:57] [main/INFO]: Loading 2 mods:";
        for invalid in [
            "\t- BAD 1.0",
            "\t- z 1.0",
            "\t- fabricated 1.0 /absolute/path",
            "\t- fabricated 1.0/path",
            "- fabricated 1.0",
            "\tat example.Loader.run(Loader.java:99)",
            "[12:34:58] [main/INFO]: - fabricated 1.0",
        ] {
            let evidence = inspect(&log(&[
                START,
                TWO,
                "\t- valid 1.0",
                invalid,
                "\t- later 1.0",
            ]));
            assert_eq!(evidence.status, FabricStatus::PartialSelection, "{invalid}");
            assert_eq!(evidence.mods.len(), 1);
            assert_eq!(evidence.mods[0].id, "valid");
            assert_eq!(evidence.omitted_count, 0);
        }

        let mut file = log(&[START, ONE, "\t- valid 1.0"]);
        file.lines[2].truncated = true;
        let evidence = inspect(&file);
        assert_eq!(evidence.status, FabricStatus::PartialSelection);
        assert!(evidence.mods.is_empty());

        file.lines[2].truncated = false;
        file.lines[2].number += 1;
        let evidence = inspect(&file);
        assert_eq!(evidence.status, FabricStatus::PartialSelection);
        assert!(evidence.mods.is_empty());
    }

    #[test]
    fn matched_count_does_not_hide_extra_or_incomplete_rows() {
        let evidence = inspect(&log(&[START, ONE, "\t- first 1.0", "\t- extra 1.0"]));
        assert_eq!(evidence.status, FabricStatus::PartialSelection);
        assert_eq!(evidence.mods.len(), 1);
        assert_eq!(evidence.omitted_count, 0);

        let mut file = log(&[START, ONE, "\t- first 1.0"]);
        assert_eq!(inspect(&file).status, FabricStatus::SelectionReported);
        file.truncated = true;
        assert_eq!(inspect(&file).status, FabricStatus::PartialSelection);
    }

    #[test]
    fn repeated_ids_do_not_establish_a_complete_selection_or_merge_rows() {
        let evidence = inspect(&log(&[
            LOOM_START,
            "[12:34:57] [main/INFO] (FabricLoader) Loading 4 mods:",
            "\t- parent_one 1.0",
            "\t   \\-- repeated 1.0",
            "\t- parent_two 2.0",
            "\t   \\-- repeated 2.0",
        ]));
        assert_eq!(evidence.status, FabricStatus::PartialSelection);
        assert_eq!(evidence.declared_count, Some(4));
        assert_eq!(evidence.mods.len(), 4);
        for (index, version, line) in [(1, "1.0", 43), (3, "2.0", 45)] {
            let entry = &evidence.mods[index];
            assert_eq!(entry.id, "repeated");
            assert_eq!(entry.version, version);
            assert_eq!(entry.line, line);
            assert!(entry.nested);
        }
        assert_eq!(evidence.omitted_count, 0);
    }

    #[test]
    fn bounds_output_and_counts_only_verified_withheld_rows() {
        let mut file = log(&[START, "[12:34:57] [main/INFO]: Loading 103 mods:"]);
        for index in 0..103 {
            file.lines.push(LogLine {
                number: index + 42,
                text: format!("\t- example_{index} 1.0"),
                truncated: false,
            });
        }
        let evidence = inspect(&file);
        assert_eq!(evidence.status, FabricStatus::SelectionReported);
        assert_eq!(evidence.mods.len(), MAX_MODS);
        assert_eq!(evidence.omitted_count, 3);

        file.lines[104].text = "\t- example_100 1.0".into();
        let evidence = inspect(&file);
        assert_eq!(evidence.status, FabricStatus::PartialSelection);
        assert_eq!(evidence.mods.len(), MAX_MODS);
        assert_eq!(evidence.omitted_count, 3);

        file.lines[104].truncated = true;
        let evidence = inspect(&file);
        assert_eq!(evidence.status, FabricStatus::PartialSelection);
        assert_eq!(evidence.mods.len(), MAX_MODS);
        assert_eq!(evidence.omitted_count, 2);

        for count in ["4097", "9999999999999999999999999999", "two"] {
            file.lines[1].text = format!("[12:34:57] [main/INFO]: Loading {count} mods:");
            let evidence = inspect(&file);
            assert_eq!(evidence.status, FabricStatus::PartialSelection);
            assert_eq!(evidence.declared_count, None);
            assert!(evidence.mods.is_empty());
            assert_eq!(evidence.omitted_count, 0);
        }
    }
}
