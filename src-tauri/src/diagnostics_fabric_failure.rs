use super::{valid_id, valid_version, DiagnosticFile, Record};
use crate::diagnostics::LogLine;

const MAX_BLOCK_LINES: usize = 64;
const MAX_DETAILS: usize = 16;
const EXCEPTION: &str = "net.fabricmc.loader.impl.FormattedException: Some of your mods are incompatible with the game or each other!";

#[derive(Clone, Copy)]
enum Locale {
    En,
    ZhCn,
}

impl Locale {
    fn from_solution_heading(heading: &str) -> Option<Self> {
        match heading {
            "A potential solution has been determined, this may resolve your problem:" => {
                Some(Self::En)
            }
            "确定了一种可能的解决方法，这样做可能会解决你的问题：" => {
                Some(Self::ZhCn)
            }
            _ => None,
        }
    }

    fn details_heading(self) -> &'static str {
        match self {
            Self::En => "More details:",
            Self::ZhCn => "更多信息：",
        }
    }

    fn is_suggestion(self, line: &str) -> bool {
        let (prefix, stop) = match self {
            Self::En => ("\t - You need to install ", '.'),
            Self::ZhCn => ("\t - 你需要安装 ", '。'),
        };
        line.strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(stop))
            .is_some_and(|rest| !rest.is_empty())
    }

    fn parse_required(self, row: &str) -> Option<RequiredDependency<'_>> {
        let (mod_label, verb) = match self {
            Self::En => ("Mod ", " requires "),
            Self::ZhCn => ("模组 ", " 需要 "),
        };
        let (from, text) = label(row.strip_prefix(mod_label).unwrap_or(row))?;
        let (version, requirement) = text.strip_prefix(' ')?.split_once(verb)?;
        if !valid_version(version) || requirement.len() > 512 {
            return None;
        }
        let (dependency, missing) = match self {
            Self::En => parse_english_requirement(requirement)?,
            Self::ZhCn => parse_chinese_requirement(requirement)?,
        };
        Some(RequiredDependency {
            from,
            version,
            dependency,
            missing,
        })
    }
}

struct RequiredDependency<'a> {
    from: &'a str,
    version: &'a str,
    dependency: &'a str,
    missing: bool,
}

pub(in crate::diagnostics) fn dependency_details(
    file: &DiagnosticFile,
    failure_line: usize,
) -> Option<Vec<(LogLine, String)>> {
    let index = file.lines.iter().position(|l| l.number == failure_line)?;
    let mut block = file.lines[index + 1..]
        .iter()
        .take(MAX_BLOCK_LINES)
        .enumerate()
        .map_while(|(i, line)| {
            (!line.truncated && failure_line.checked_add(i + 1) == Some(line.number))
                .then_some(line)
        });

    // The resolver's short explanations can use the same code for missing and
    // mismatched dependencies. Only classify the subsequent exception details.
    for prefix in ["Immediate reason: [", "Reason: ["] {
        let message = resolution_info(block.next()?)?;
        if !message.starts_with(prefix) || !message.ends_with(']') {
            return None;
        }
    }
    let mut outcome = resolution_info(block.next()?)?;
    if outcome.starts_with("CLIENT environment disabled: [")
        || outcome.starts_with("SERVER environment disabled: [")
    {
        outcome = resolution_info(block.next()?)?;
    }
    let has_fix = outcome.starts_with("Fix: add [")
        && outcome.contains("], remove [")
        && outcome.contains("], replace [")
        && outcome.ends_with(']');
    if !has_fix && outcome != "No fix?" {
        return None;
    }
    let headline = Record::parse(&block.next()?.text)?;
    if !headline.is("ERROR", "FabricLoader")
        || headline.message != "Incompatible mods found!"
        || block.next()?.text != EXCEPTION
    {
        return None;
    }

    let mut line = block.next()?;
    let mut locale = None;
    if has_fix {
        let selected = Locale::from_solution_heading(&line.text)?;
        locale = Some(selected);
        // Skip the solution text; it is neither used as evidence nor actionable.
        line = block.next()?;
        if !line.text.starts_with("\t - ") {
            return None;
        }
        while line.text.starts_with("\t - ") || line.text.starts_with("\t\t - ") {
            line = block.next()?;
        }
        if line.text != selected.details_heading() {
            return None;
        }
        line = block.next()?;
    }

    let prefix = if has_fix { "\t - " } else { " - " };
    let mut findings = Vec::new();
    let mut seen = std::collections::HashSet::new();
    while let Some(row) = line.text.strip_prefix(prefix) {
        let (selected, detail) = match locale {
            Some(selected) => (selected, selected.parse_required(row)?),
            // No solution header: infer the locale from the first detail and
            // require all subsequent evidence to use that same grammar.
            None => [Locale::En, Locale::ZhCn]
                .into_iter()
                .find_map(|locale| Some((locale, locale.parse_required(row)?)))?,
        };
        locale = Some(selected);
        let RequiredDependency {
            from,
            version,
            dependency,
            missing,
        } = detail;
        if findings.len() == MAX_DETAILS || !seen.insert((from, version, dependency)) {
            return None;
        }
        let summary = if missing {
            format!("Fabric 日志记录：{from} {version} 所需的依赖 {dependency} 缺失；版本要求见原始证据")
        } else {
            format!("Fabric 日志记录：{from} {version} 所需的依赖 {dependency} 版本不匹配；要求及已发现版本见原始证据")
        };
        findings.push((line.clone(), summary));
        if !has_fix && !selected.is_suggestion(&block.next()?.text) {
            return None;
        }
        line = block.next()?;
    }
    // A recognized stack transition is required; EOF isn't a block boundary.
    if findings.is_empty()
        || !frame(
            line,
            "FormattedException.ofLocalized",
            "FormattedException.java:",
        )
        || !frame(
            block.next()?,
            "FabricLoaderImpl.load",
            "FabricLoaderImpl.java:",
        )
    {
        return None;
    }
    Some(findings)
}

fn resolution_info(line: &LogLine) -> Option<&str> {
    let record = Record::parse(&line.text)?;
    record
        .is("INFO", "FabricLoader/Resolution")
        .then_some(record.message)
}

fn frame(line: &LogLine, method: &str, source: &str) -> bool {
    let Some((location, suffix)) = line
        .text
        .strip_prefix("\tat net.fabricmc.loader.impl.")
        .and_then(|frame| frame.strip_prefix(method))
        .and_then(|frame| frame.strip_prefix('('))
        .and_then(|frame| frame.split_once(')'))
    else {
        return false;
    };
    location
        .strip_prefix(source)
        .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
        && (suffix.is_empty()
            || (matches!(suffix.get(..2), Some(" [" | " ~")) && suffix.ends_with(']')))
}

fn parse_english_requirement(requirement: &str) -> Option<(&str, bool)> {
    let (range, target) = requirement.split_once(" of ")?;
    if !valid_english_range(range) {
        return None;
    }
    if let Some(dependency) = target.strip_suffix(", which is missing!") {
        return valid_id(dependency).then_some((dependency, true));
    }
    let (dependency, text) = label(target.strip_prefix("mod ").unwrap_or(target))?;
    let (versions, plural) =
        if let Some(versions) = text.strip_prefix(", but only the wrong version is present: ") {
            (versions, false)
        } else {
            (
                text.strip_prefix(", but only the wrong versions are present: ")?,
                true,
            )
        };
    let versions = versions.strip_suffix('!')?;
    (versions.contains('/') == plural && versions.split('/').all(valid_version))
        .then_some((dependency, false))
}

fn parse_chinese_requirement(requirement: &str) -> Option<(&str, bool)> {
    // The Chinese bundle puts the dependency before the range. Consume a
    // display name first so a localized name containing " 的 " isn't split.
    if let Some((dependency, tail)) =
        label(requirement.strip_prefix("模组 ").unwrap_or(requirement))
    {
        let (range, versions) = tail
            .strip_prefix(" 的 ")?
            .split_once("，但已经安装了的版本 ")?;
        let versions = versions.strip_suffix(" 不对！")?;
        return (valid_chinese_range(range) && versions.split('/').all(valid_version))
            .then_some((dependency, false));
    }
    let (dependency, range) = requirement
        .strip_suffix("，但没有安装它！")?
        .split_once(" 的 ")?;
    (valid_id(dependency) && valid_chinese_range(range)).then_some((dependency, true))
}

// Only a single interval rendering is interpreted. The range is
// kept in the evidence, never converted to a selector or solved here.
fn valid_english_range(range: &str) -> bool {
    if range == "any version" {
        return true;
    }
    if let Some(version) = range.strip_prefix("version ") {
        return valid_version(
            version
                .strip_suffix(" or later")
                .or_else(|| version.strip_suffix(" or earlier"))
                .unwrap_or(version),
        );
    }
    if let Some(interval) = range.strip_prefix("any version between ") {
        return interval.split_once(" and ").is_some_and(|(lower, upper)| {
            [lower, upper].into_iter().all(|bound| {
                [" (inclusive)", " (exclusive)"]
                    .into_iter()
                    .any(|suffix| bound.strip_suffix(suffix).is_some_and(valid_version))
            })
        });
    }
    if let Some(wildcard) = range
        .strip_prefix("any ")
        .and_then(|r| r.strip_suffix(".x version"))
    {
        return valid_wildcard_prefix(wildcard);
    }
    ["any version before ", "any version after "]
        .into_iter()
        .any(|prefix| range.strip_prefix(prefix).is_some_and(valid_version))
}

fn valid_chinese_range(range: &str) -> bool {
    if let Some(interval) = range
        .strip_prefix("从 ")
        .and_then(|r| r.strip_suffix("的任意版本"))
    {
        return interval.split_once("到 ").is_some_and(|(lower, upper)| {
            [lower, upper].into_iter().all(|bound| {
                ["（含）", "（不含）"]
                    .into_iter()
                    .any(|suffix| bound.strip_suffix(suffix).is_some_and(valid_version))
            })
        });
    }
    if let Some(wildcard) = range
        .strip_prefix("任意 ")
        .and_then(|r| r.strip_suffix(".x 版本"))
    {
        return valid_wildcard_prefix(wildcard);
    }
    range == "任意版本"
        || [
            " 版本",
            " 以上任意版本",
            " 及以上版本",
            " 以下任意版本",
            " 及以下版本",
        ]
        .into_iter()
        .any(|suffix| range.strip_suffix(suffix).is_some_and(valid_version))
}

fn valid_wildcard_prefix(version: &str) -> bool {
    let mut parts = version.split('.');
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    parts.next().is_some_and(digits) && parts.next().is_none_or(digits) && parts.next().is_none()
}

fn label(text: &str) -> Option<(&str, &str)> {
    let (name, rest) = text.strip_prefix('\'')?.split_once("' (")?;
    let (id, tail) = rest.split_once(')')?;
    (!name.trim().is_empty() && name.len() <= 128 && valid_id(id)).then_some((id, tail))
}
