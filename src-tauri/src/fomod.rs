//! Read-only FOMOD configuration previews.
//!
//! Exposes file mappings and selectable groups for an installation review.
//! Only deterministic configurations without state-dependent rules are preselected.

use crate::{
    archive,
    error::{AppError, AppResult},
    paths, plugins, safe_fs,
    transaction::FileEngine,
    util,
};
use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_CONFIG_BYTES: usize = 256 * 1024;
const MAX_GROUPS: usize = 128;
const MAX_OPTIONS: usize = 512;
const MAX_MAPPINGS: usize = 10_000;
const MAX_TEXT: usize = 2048;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FomodMapping {
    pub source: String,
    pub destination: String,
    pub priority: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FomodOption {
    pub name: String,
    pub description: Option<String>,
    pub image: Option<String>,
    pub mappings: Vec<FomodMapping>,
    pub has_conditions: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FomodGroup {
    pub name: String,
    pub selection_type: String,
    pub options: Vec<FomodOption>,
    pub has_conditions: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FomodAutoOption {
    pub group: String,
    pub group_index: usize,
    pub option: String,
    pub option_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FomodAutoSelection {
    /// `safe` means every group was resolved by a deterministic rule that did
    /// not evaluate a condition or execute a script. `requires_user` means
    /// the preview is useful evidence but must not be sent as install mappings.
    pub status: String,
    pub selected_options: Vec<FomodAutoOption>,
    pub mappings: Vec<FomodMapping>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FomodPreview {
    #[serde(serialize_with = "crate::paths::serialize_user_path")]
    pub artifact_path: PathBuf,
    pub artifact_sha256: String,
    pub config_path: String,
    pub module_name: Option<String>,
    pub required_mappings: Vec<FomodMapping>,
    pub groups: Vec<FomodGroup>,
    pub automatic_selection: FomodAutoSelection,
    pub warnings: Vec<String>,
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::Invalid(message.into())
}

fn bounded_text(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    let mut chars = value.chars();
    let text: String = chars.by_ref().take(MAX_TEXT).collect();
    Some(if chars.next().is_some() {
        format!("{text}…")
    } else {
        text
    })
}

fn required_attr<'a>(node: Node<'a, 'a>, name: &str) -> AppResult<&'a str> {
    node.attribute(name)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid(format!("FOMOD {name} 属性缺失")))
}

fn relative(value: &str, label: &str) -> AppResult<String> {
    let normalized = value.replace('\\', "/");
    let path = paths::safe_join(Path::new(""), &normalized)
        .map_err(|error| invalid(format!("FOMOD {label} 路径无效: {error}")))?;
    let text = path
        .to_str()
        .ok_or_else(|| invalid(format!("FOMOD {label} 路径不是 UTF-8")))?
        .replace('\\', "/");
    if text.is_empty() {
        return Err(invalid(format!("FOMOD {label} 路径为空")));
    }
    Ok(text)
}

fn join_relative(prefix: &str, tail: &str) -> String {
    if prefix.is_empty() {
        tail.to_owned()
    } else {
        format!("{prefix}/{tail}")
    }
}

struct ArchiveMappings {
    module_root: String,
    files: BTreeMap<String, String>,
    remaining: usize,
    declarations: usize,
}

impl ArchiveMappings {
    fn new(config_path: &str, archive_files: impl IntoIterator<Item = String>) -> Self {
        // A wrapper that contains fomod/ is the base for every XML source,
        // while the install request uses complete archive-relative names.
        let module_root = config_path
            .rsplit_once('/')
            .and_then(|(parent, _)| parent.rsplit_once('/'))
            .filter(|(_, config_dir)| config_dir.eq_ignore_ascii_case("fomod"))
            .map_or_else(String::new, |(wrapper, _)| wrapper.to_owned());
        Self {
            module_root,
            // The verified archive inventory rejects case aliases. Keep the
            // exact entry spelling for subsequent install and hashing.
            files: archive_files
                .into_iter()
                .map(|file| (file.to_lowercase(), file))
                .collect(),
            remaining: MAX_MAPPINGS,
            declarations: 0,
        }
    }

    fn append(&mut self, node: Node<'_, '_>, result: &mut Vec<FomodMapping>) -> AppResult<()> {
        self.declarations += 1;
        let source = relative(required_attr(node, "source")?, "source")?;
        if source
            .split('/')
            .next()
            .is_some_and(|part| part.eq_ignore_ascii_case("fomod"))
        {
            return Err(invalid("FOMOD 安装载荷不能来自安装器元数据目录"));
        }
        let raw_destination = node.attribute("destination").unwrap_or(&source);
        let destination = if raw_destination.is_empty() {
            String::new()
        } else {
            relative(raw_destination, "destination")?
        };
        let priority = node
            .attribute("priority")
            .map(|value| {
                value
                    .parse::<i32>()
                    .map_err(|_| invalid("FOMOD priority 必须是整数"))
            })
            .transpose()?;
        let source_key = join_relative(&self.module_root, &source).to_lowercase();

        let remaining = &mut self.remaining;
        let mut push = |source: &str, destination: String| -> AppResult<()> {
            *remaining = remaining
                .checked_sub(1)
                .ok_or_else(|| invalid("FOMOD 展开后的文件映射超过上限"))?;
            plugins::validate_relative_path(source, false)?;
            plugins::validate_relative_path(&destination, false)?;
            result.push(FomodMapping {
                source: source.to_owned(),
                destination,
                priority,
            });
            Ok(())
        };

        if node.has_tag_name("file") {
            let archive_name = self
                .files
                .get(&source_key)
                .ok_or_else(|| invalid(format!("FOMOD source 不在归档中: {source}")))?;
            let leaf = source.rsplit('/').next().unwrap_or(&source);
            let destination = if destination.is_empty() || raw_destination.ends_with(['/', '\\']) {
                join_relative(&destination, leaf)
            } else {
                destination
            };
            return push(archive_name, destination);
        }

        // Use a slash-terminated prefix so Core does not match CoreExtra.
        let prefix = format!("{source_key}/");
        let depth = prefix.bytes().filter(|byte| *byte == b'/').count();
        let mut entries = self
            .files
            .range(prefix.clone()..)
            .take_while(|(key, _)| key.starts_with(&prefix))
            .peekable();
        if entries.peek().is_none() {
            return Err(invalid(format!(
                "FOMOD source 目录不存在或没有文件: {source}"
            )));
        }
        for (_, archive_name) in entries {
            // Lowercasing can change byte lengths. Strip by path components
            // from the original entry name to preserve its spelling.
            let suffix = archive_name
                .splitn(depth + 1, '/')
                .last()
                .unwrap_or(archive_name);
            push(archive_name, join_relative(&destination, suffix))?;
        }
        Ok(())
    }

    fn under(&mut self, node: Node<'_, '_>) -> AppResult<Vec<FomodMapping>> {
        let mut result = Vec::new();
        for child in node.children() {
            if child.has_tag_name("file") || child.has_tag_name("folder") {
                self.append(child, &mut result)?;
            } else if child.has_tag_name("files") {
                for entry in child
                    .children()
                    .filter(|entry| entry.has_tag_name("file") || entry.has_tag_name("folder"))
                {
                    self.append(entry, &mut result)?;
                }
            }
        }
        Ok(result)
    }
}

fn has_conditions(node: Node<'_, '_>) -> bool {
    node.descendants().any(|child| {
        if !child.is_element() {
            return false;
        }
        match child.tag_name().name() {
            "visible"
            | "moduleDependencies"
            | "conditionalFileInstalls"
            | "dependencies"
            | "flagDependency"
            | "fileDependency"
            | "gameDependency"
            | "conditionFlags"
            | "dependencyType"
            | "script" => true,
            "typeDescriptor" => {
                let mut elements = child.children().filter(Node::is_element);
                let static_type = elements.next().is_some_and(|element| {
                    element.has_tag_name("type")
                        && matches!(
                            element.attribute("name"),
                            Some("Optional" | "Recommended" | "Required")
                        )
                });
                !static_type || elements.next().is_some()
            }
            "file" | "folder" => {
                ["alwaysInstall", "installIfUsable"]
                    .into_iter()
                    .any(|attribute| {
                        child
                            .attribute(attribute)
                            .is_some_and(|value| !matches!(value, "false" | "0"))
                    })
            }
            _ => false,
        }
    })
}

fn automatic_selection(
    required_mappings: &[FomodMapping],
    groups: &[FomodGroup],
    configuration_has_conditions: bool,
) -> FomodAutoSelection {
    let mut selected_options = Vec::new();
    let mut mappings = required_mappings.to_vec();
    let mut warnings = Vec::new();
    if configuration_has_conditions {
        warnings.push("FOMOD 含条件、依赖或额外安装规则；需要用户核对文件映射".into());
    }

    for (group_index, group) in groups.iter().enumerate() {
        let selected = match group.selection_type.as_str() {
            // Selecting every option is deterministic only when no option is
            // guarded by a dependency or a script-like descriptor.
            "SelectAll"
                if !group.has_conditions
                    && group.options.iter().all(|option| !option.has_conditions) =>
            {
                Some((0..group.options.len()).collect::<Vec<_>>())
            }
            // A one-option required group is safe without interpreting any
            // conditions. More complex groups remain explicit user choices.
            "SelectOne" | "SelectExactlyOne" | "SelectAtLeastOne"
                if !group.has_conditions
                    && group.options.len() == 1
                    && !group.options[0].has_conditions =>
            {
                Some(vec![0])
            }
            _ => None,
        };
        let Some(selected) = selected else {
            warnings.push(format!(
                "FOMOD 组 {} 不能由安全规则唯一选择；需要用户确认",
                group.name
            ));
            continue;
        };
        for option_index in selected {
            let option = &group.options[option_index];
            selected_options.push(FomodAutoOption {
                group: group.name.clone(),
                group_index,
                option: option.name.clone(),
                option_index,
            });
            mappings.extend(option.mappings.clone());
        }
    }

    // Even a deterministic selection must not silently choose between two
    // files that target the same destination. Explicit priority semantics are
    // intentionally left for a future reviewed mapping flow.
    let mut sources = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    for mapping in &mappings {
        if !sources.insert(&mapping.source) {
            warnings.push(format!(
                "自动选择包含重复归档源文件 {}；需要用户确认",
                mapping.source
            ));
        }
        let key = mapping.destination.to_lowercase();
        if !destinations.insert(key) {
            warnings.push(format!(
                "自动选择包含重复安装目标 {}；需要用户确认",
                mapping.destination
            ));
        }
    }

    for destination in &destinations {
        if destination
            .match_indices('/')
            .any(|(index, _)| destinations.contains(&destination[..index]))
        {
            warnings.push(format!(
                "自动选择的安装目标 {destination} 与上级文件冲突；需要用户确认"
            ));
        }
    }
    if mappings.is_empty() {
        warnings.push("FOMOD 没有可自动选择的文件映射".into());
    }

    let complete = warnings.is_empty();
    if !complete {
        selected_options.clear();
        mappings = required_mappings.to_vec();
    }

    FomodAutoSelection {
        status: if complete {
            "safe".into()
        } else {
            "requires_user".into()
        },
        selected_options,
        mappings,
        warnings,
    }
}

fn find_config(files: &BTreeMap<PathBuf, String>) -> AppResult<PathBuf> {
    let mut candidates: Vec<_> = files
        .keys()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("ModuleConfig.xml"))
        })
        .cloned()
        .collect();
    candidates.sort();
    if candidates.is_empty() {
        return Err(AppError::NotFound("归档没有 FOMOD/ModuleConfig.xml".into()));
    }
    let preferred: Vec<_> = candidates
        .iter()
        .filter(|path| {
            path.parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name.eq_ignore_ascii_case("fomod"))
        })
        .collect();
    if preferred.len() == 1 {
        return Ok(preferred[0].clone());
    }
    if candidates.len() != 1 {
        return Err(invalid("归档包含多个 FOMOD ModuleConfig.xml，无法安全选择"));
    }
    Ok(candidates[0].clone())
}

fn parse_config(
    bytes: &[u8],
    config_path: String,
    artifact_path: PathBuf,
    artifact_sha256: String,
    archive_files: impl IntoIterator<Item = String>,
) -> AppResult<FomodPreview> {
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(invalid("FOMOD ModuleConfig.xml 超过 256 KiB"));
    }
    if bytes
        .windows(9)
        .any(|window| window.eq_ignore_ascii_case(b"<!doctype"))
    {
        return Err(invalid("FOMOD 配置含 DOCTYPE，拒绝解析外部实体"));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| invalid("FOMOD ModuleConfig.xml 必须是 UTF-8 XML"))?;
    let document =
        Document::parse(text).map_err(|error| invalid(format!("FOMOD XML 无效: {error}")))?;
    let root = document.root_element();
    if root.tag_name().name() != "config" {
        return Err(invalid("FOMOD XML 根元素不是 config"));
    }
    let module_name = root
        .children()
        .find(|child| child.is_element() && child.tag_name().name() == "moduleName")
        .and_then(|node| bounded_text(node.text()));
    let mut warnings = Vec::new();
    let mut archive_mappings = ArchiveMappings::new(&config_path, archive_files);
    let required_mappings = root
        .children()
        .find(|child| child.is_element() && child.tag_name().name() == "requiredInstallFiles")
        .map(|node| archive_mappings.under(node))
        .transpose()?
        .unwrap_or_default();
    let steps = root
        .children()
        .find(|child| child.is_element() && child.tag_name().name() == "installSteps");
    let mut groups = Vec::new();
    if let Some(steps) = steps {
        for step in steps
            .children()
            .filter(|child| child.is_element() && child.tag_name().name() == "installStep")
        {
            for group in step
                .descendants()
                .filter(|child| child.is_element() && child.tag_name().name() == "group")
            {
                if groups.len() >= MAX_GROUPS {
                    return Err(invalid("FOMOD 可选组超过上限"));
                }
                let mut options = Vec::new();
                if let Some(plugins) = group
                    .children()
                    .find(|child| child.is_element() && child.tag_name().name() == "plugins")
                {
                    for plugin in plugins
                        .children()
                        .filter(|child| child.is_element() && child.tag_name().name() == "plugin")
                    {
                        if options.len() >= MAX_OPTIONS {
                            return Err(invalid("FOMOD 选项超过上限"));
                        }
                        options.push(FomodOption {
                            name: bounded_text(plugin.attribute("name"))
                                .unwrap_or_else(|| "未命名选项".into()),
                            description: bounded_text(
                                plugin
                                    .children()
                                    .find(|child| {
                                        child.is_element()
                                            && child.tag_name().name() == "description"
                                    })
                                    .and_then(|child| child.text()),
                            ),
                            image: plugin
                                .children()
                                .find(|child| {
                                    child.is_element() && child.tag_name().name() == "image"
                                })
                                .and_then(|child| child.attribute("path"))
                                .map(|value| {
                                    relative(value, "image").unwrap_or_else(|_| value.to_string())
                                }),
                            mappings: archive_mappings.under(plugin)?,
                            has_conditions: has_conditions(plugin),
                        });
                    }
                }
                let selection_type = group.attribute("type").unwrap_or("SelectAny").to_string();
                if selection_type != "SelectAny" && selection_type != "SelectAll" {
                    warnings.push(format!(
                        "FOMOD 组 {} 使用 {} 选择约束；安装前仍需用户确认",
                        bounded_text(group.attribute("name")).unwrap_or_else(|| "未命名组".into()),
                        selection_type
                    ));
                }
                if options.iter().any(|option| option.has_conditions) {
                    warnings.push(format!(
                        "FOMOD 组 {} 含条件或依赖；未根据游戏状态自动选择",
                        bounded_text(group.attribute("name")).unwrap_or_else(|| "未命名组".into())
                    ));
                }
                groups.push(FomodGroup {
                    name: bounded_text(group.attribute("name"))
                        .unwrap_or_else(|| "未命名组".into()),
                    selection_type,
                    options,
                    has_conditions: has_conditions(group)
                        || step.children().any(|child| child.has_tag_name("visible")),
                });
            }
        }
    }
    if groups.is_empty() && required_mappings.is_empty() {
        warnings.push("FOMOD 配置没有可审查的文件映射".into());
    }
    // A file directive outside the supported sections cannot be silently
    // omitted when the caller requests an automatic plan.
    let declarations = root
        .descendants()
        .filter(|node| node.has_tag_name("file") || node.has_tag_name("folder"))
        .count();
    let automatic_selection = automatic_selection(
        &required_mappings,
        &groups,
        has_conditions(root) || archive_mappings.declarations != declarations,
    );
    Ok(FomodPreview {
        artifact_path,
        artifact_sha256,
        config_path,
        module_name,
        required_mappings,
        groups,
        automatic_selection,
        warnings,
    })
}

fn artifact_path(engine: &FileEngine, path: &Path) -> AppResult<PathBuf> {
    engine.paths.stable_directory(&engine.paths.root)?;
    engine.paths.stable_directory(&engine.paths.downloads())?;
    engine.paths.stable_directory(&engine.paths.staging())?;
    engine.paths.guard(path, &[])?;
    let path = paths::ensure_within(path, &[engine.paths.downloads(), engine.paths.staging()])?;
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.file_type().is_file() {
        return Err(AppError::Denied("FOMOD 预览载荷必须是普通文件".into()));
    }
    Ok(path)
}

pub fn preview(engine: &FileEngine, requested_path: &Path) -> AppResult<FomodPreview> {
    let artifact = artifact_path(engine, requested_path)?;
    let digest = util::sha256_file(&artifact)?;
    let preview_root = engine.paths.staging().join("fomod-previews");
    safe_fs::create_dir_all(&preview_root)?;
    let destination = engine
        .paths
        .staging()
        .join("fomod-previews")
        .join(uuid::Uuid::new_v4().to_string());
    let extracted = archive::extract_verified_archive(&artifact, &destination, &digest)?;
    preview_extracted(&extracted, &destination, artifact, digest)
}

pub(crate) fn preview_extracted(
    extracted: &archive::VerifiedExtraction,
    destination: &Path,
    artifact: PathBuf,
    digest: String,
) -> AppResult<FomodPreview> {
    let config_path = find_config(&extracted.files)?;
    let display_path = config_path
        .strip_prefix(destination)
        .map_err(|_| invalid("FOMOD 配置不在预览目录内"))?
        .to_string_lossy()
        .replace('\\', "/");
    let mut bytes = Vec::new();
    safe_fs::open_regular(&config_path)?
        .take((MAX_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(invalid("FOMOD ModuleConfig.xml 超过 256 KiB"));
    }
    if util::sha256_reader(&mut bytes.as_slice())? != extracted.files[&config_path] {
        return Err(AppError::Conflict(
            "FOMOD 配置已变化，请重新扫描归档".into(),
        ));
    }
    let archive_files = extracted
        .files
        .keys()
        .filter_map(|path| path.strip_prefix(destination).ok())
        .filter_map(Path::to_str)
        .map(|path| path.replace('\\', "/"));
    let mut preview = parse_config(&bytes, display_path, artifact, digest, archive_files)?;
    if extracted.files.keys().any(|path| {
        path.parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name.eq_ignore_ascii_case("fomod"))
            && path.file_name().is_some_and(|name| {
                ["script.cs", "script.txt", "script.xml"]
                    .iter()
                    .any(|script| name.eq_ignore_ascii_case(script))
            })
    }) {
        let selection = &mut preview.automatic_selection;
        selection.status = "requires_user".into();
        selection.selected_options.clear();
        selection.mappings = preview.required_mappings.clone();
        selection
            .warnings
            .push("FOMOD 含额外安装脚本；需要用户核对文件映射".into());
    }
    Ok(preview)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::Db, storage::AppPaths};
    use std::io::Write;

    #[test]
    fn parses_required_files_groups_and_conditions_without_selecting() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <moduleName>Example</moduleName>
  <requiredInstallFiles><file source="Data/base.esp" destination="base.esp" priority="1"/></requiredInstallFiles>
  <installSteps order="Explicit"><installStep name="Main">
    <optionalFileGroups order="SelectExactlyOne"><group name="Texture" type="SelectExactlyOne">
      <plugins order="Explicit"><plugin name="A"><description>Choice A</description><files><file source="A/a.dds" destination="textures/a.dds"/></files></plugin>
      <plugin name="B"><files><file source="B/b.dds" destination="textures/a.dds"/></files><dependencies><flagDependency flag="x" value="y"/></dependencies></plugin></plugins>
    </group></optionalFileGroups>
  </installStep></installSteps>
</config>"#;
        let preview = parse_config(
            xml,
            "fomod/ModuleConfig.xml".into(),
            PathBuf::from("artifact.zip"),
            "a".repeat(64),
            ["Data/base.esp", "A/a.dds", "B/b.dds"].map(String::from),
        )
        .unwrap();
        assert_eq!(preview.module_name.as_deref(), Some("Example"));
        assert_eq!(preview.required_mappings[0].destination, "base.esp");
        assert_eq!(preview.groups.len(), 1);
        assert_eq!(preview.groups[0].selection_type, "SelectExactlyOne");
        assert_eq!(preview.groups[0].options.len(), 2);
        assert!(preview.groups[0].options[1].has_conditions);
        assert_eq!(preview.automatic_selection.status, "requires_user");
        assert!(!preview.warnings.is_empty());
    }

    #[test]
    fn auto_selects_only_deterministic_groups_and_keeps_duplicate_targets_manual() {
        let xml = br#"<config>
  <requiredInstallFiles><file source="base.esp" destination="base.esp"/></requiredInstallFiles>
  <installSteps><installStep><optionalFileGroups>
    <group name="Defaults" type="SelectAll"><plugins>
      <plugin name="A"><files><file source="a.esp" destination="a.esp"/></files><typeDescriptor><type name="Required"/></typeDescriptor></plugin>
      <plugin name="B"><files><file source="b.esp" destination="b.esp"/></files></plugin>
    </plugins></group>
    <group name="Defaults" type="SelectExactlyOne"><plugins>
      <plugin name="Only"><files><file source="only.esp" destination="only.esp"/></files></plugin>
    </plugins></group>
  </optionalFileGroups></installStep></installSteps>
</config>"#;
        let preview = parse_config(
            xml,
            "fomod/ModuleConfig.xml".into(),
            PathBuf::from("artifact.zip"),
            "e".repeat(64),
            ["base.esp", "a.esp", "b.esp", "only.esp"].map(String::from),
        )
        .unwrap();
        assert_eq!(preview.automatic_selection.status, "safe");
        assert_eq!(preview.automatic_selection.selected_options.len(), 3);
        assert_eq!(
            preview.automatic_selection.selected_options[2].group_index,
            1
        );
        assert_eq!(preview.automatic_selection.mappings.len(), 4);

        let mut duplicate = preview.groups.clone();
        duplicate[1].options[0].mappings[0].destination = "a.esp".into();
        let selection = automatic_selection(&preview.required_mappings, &duplicate, false);
        assert_eq!(selection.status, "requires_user");
        assert!(selection.selected_options.is_empty());
        assert_eq!(selection.mappings.len(), 1);
        assert!(selection
            .warnings
            .iter()
            .any(|warning| warning.contains("重复安装目标")));

        duplicate[1].options[0].mappings[0].destination = "a.esp/file".into();
        let selection = automatic_selection(&preview.required_mappings, &duplicate, false);
        assert_eq!(selection.status, "requires_user");
        assert!(selection
            .warnings
            .iter()
            .any(|warning| warning.contains("上级文件冲突")));
    }

    #[test]
    fn state_dependent_rules_prevent_preselection_across_the_configuration() {
        for (root_rule, step_rule, option_rule) in [
            (
                "<moduleDependencies><gameDependency version='1'/></moduleDependencies>",
                "",
                "",
            ),
            (
                "<conditionalFileInstalls><patterns/></conditionalFileInstalls>",
                "",
                "",
            ),
            (
                "<unrecognized><file source='b.esp' destination='b.esp'/></unrecognized>",
                "",
                "",
            ),
            (
                "",
                "<visible><flagDependency flag='patch' value='yes'/></visible>",
                "",
            ),
            (
                "",
                "",
                "<typeDescriptor><type name='NotUsable'/></typeDescriptor>",
            ),
            (
                "",
                "",
                "<conditionFlags><flag name='patch'>yes</flag></conditionFlags>",
            ),
            (
                "",
                "",
                "<file source='b.esp' destination='b.esp' alwaysInstall='true'/>",
            ),
            (
                "",
                "",
                "<folder source='bonus' destination='' installIfUsable='true'/>",
            ),
        ] {
            let xml = format!("<config>{root_rule}<installSteps><installStep>{step_rule}<optionalFileGroups>\
              <group name='Core' type='SelectAll'><plugins><plugin name='Core'>\
              <files><file source='a.esp' destination='a.esp'/></files>{option_rule}</plugin></plugins></group>\
              </optionalFileGroups></installStep></installSteps></config>");
            let preview = parse_config(
                xml.as_bytes(),
                "ModuleConfig.xml".into(),
                PathBuf::from("artifact.zip"),
                "a".repeat(64),
                ["a.esp", "b.esp", "bonus/c.esp"].map(String::from),
            )
            .unwrap();
            assert_eq!(preview.automatic_selection.status, "requires_user", "{xml}");
            assert!(preview.automatic_selection.selected_options.is_empty());
            assert!(preview.automatic_selection.mappings.is_empty());
        }
    }

    #[test]
    fn rejects_external_entities_and_unsafe_paths() {
        let result = parse_config(
            br#"<!DOCTYPE config><config/>"#,
            "fomod/ModuleConfig.xml".into(),
            PathBuf::from("artifact.zip"),
            "b".repeat(64),
            [],
        );
        assert!(result.is_err());
        let result = relative("../escape.esp", "source");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_mapping_sources_not_present_in_extracted_archive() {
        for entry in [
            "<file source='missing.esp'/>",
            "<folder source='texture'/>",
            "<file source='textures'/>",
            "<folder source='plugin.esp'/>",
        ] {
            let xml = format!("<requiredInstallFiles>{entry}</requiredInstallFiles>");
            let document = Document::parse(&xml).unwrap();
            let mut files = ArchiveMappings::new(
                "fomod/ModuleConfig.xml",
                ["plugin.esp", "textures/a.dds"].map(String::from),
            );
            assert!(files.under(document.root_element()).is_err(), "{entry}");
        }
    }

    #[test]
    fn expands_folder_defaults_and_preserves_exact_archive_spelling() {
        let xml = br#"<config><requiredInstallFiles>
            <folder source="core\" destination="" priority="1"/>
            <folder source="Patches"/>
            <file source="Loose/third.esp" destination="Extras\"/>
            <file source="Loose/fourth.esp" destination=""/>
        </requiredInstallFiles></config>"#;
        let preview = parse_config(
            xml,
            "Release/FOMOD/ModuleConfig.xml".into(),
            PathBuf::from("artifact.zip"),
            "c".repeat(64),
            [
                "Release/Core/Textures/First.dds",
                "Release/CoreExtra/unselected.esp",
                "Release/Patches/second.esp",
                "Release/Loose/third.esp",
                "Release/Loose/fourth.esp",
                "Core/other.esp",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(preview.automatic_selection.status, "safe");
        let pairs: Vec<_> = preview
            .required_mappings
            .iter()
            .map(|mapping| {
                (
                    mapping.source.as_str(),
                    mapping.destination.as_str(),
                    mapping.priority,
                )
            })
            .collect();
        assert_eq!(
            pairs,
            [
                (
                    "Release/Core/Textures/First.dds",
                    "Textures/First.dds",
                    Some(1)
                ),
                ("Release/Patches/second.esp", "Patches/second.esp", None),
                ("Release/Loose/third.esp", "Extras/third.esp", None),
                ("Release/Loose/fourth.esp", "fourth.esp", None),
            ]
        );
    }

    #[test]
    fn refuses_folder_escape_and_budgets_expansion_across_options() {
        let inventory = ["core/a.esp", "core/nested/b.dds", "fomod/info.xml"].map(String::from);
        for entry in [
            "<folder source='../core' destination='textures'/>",
            "<folder source='core' destination='textures/..'/>",
            "<folder source='core' destination='\\textures'/>",
            "<folder source='fomod' destination=''/>",
            "<folder source='' destination=''/>",
        ] {
            let xml = format!("<files>{entry}</files>");
            let document = Document::parse(&xml).unwrap();
            let mut files = ArchiveMappings::new("fomod/ModuleConfig.xml", inventory.clone());
            assert!(files.under(document.root_element()).is_err(), "{entry}");
        }
        let document =
            Document::parse("<files><folder source='core' destination='x'/></files>").unwrap();
        let mut files = ArchiveMappings::new("fomod/ModuleConfig.xml", inventory);
        files.remaining = 3;
        assert_eq!(files.under(document.root_element()).unwrap().len(), 2);
        let error = files.under(document.root_element()).unwrap_err();
        assert!(error.to_string().contains("展开后的文件映射超过上限"));
    }

    #[test]
    fn previews_a_verified_archive_without_touching_a_game_root() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("data")).unwrap();
        let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths.clone());
        let artifact = paths.downloads().join("fixture.zip");
        let file = fs::File::create(&artifact).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("fomod/ModuleConfig.xml", options).unwrap();
        zip.write_all(
            br#"<config><moduleName>Fixture</moduleName><requiredInstallFiles><file source="Data/base.esp" destination="base.esp"/></requiredInstallFiles></config>"#,
        )
        .unwrap();
        zip.start_file("Data/base.esp", options).unwrap();
        zip.write_all(b"plugin").unwrap();
        zip.finish().unwrap();

        let preview = preview(&engine, &artifact).unwrap();
        assert_eq!(preview.module_name.as_deref(), Some("Fixture"));
        assert_eq!(preview.config_path, "fomod/ModuleConfig.xml");
        assert_eq!(preview.required_mappings[0].source, "Data/base.esp");
        assert!(!paths.root.join("game").exists());
    }
}
