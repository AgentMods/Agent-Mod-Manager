use super::*;

/// This path is *only* valid in the wire protocol. Private storage is derived
/// from the validated digest, never from this field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Blob {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}
impl Blob {
    pub fn from_digest(sha256: String, size: u64) -> Self {
        Self {
            path: format!("inputs/{sha256}"),
            sha256,
            size,
        }
    }
    pub fn fp(&self) -> Fingerprint {
        Fingerprint {
            kind: "file".into(),
            size: self.size,
            sha256: self.sha256.clone(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Asset {
    pub name: String,
    pub source: Blob,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum Contribution {
    Dlc {
        pack_name: String,
        source: Blob,
    },
    Gameconfig {
        source: Blob,
    },
    Vehicle {
        files: Vec<Asset>,
    },
    Ped {
        model_name: String,
        sex: String,
        streamed: bool,
        files: Vec<Asset>,
    },
}
impl Contribution {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Dlc { .. } => "dlc",
            Self::Gameconfig { .. } => "gameconfig",
            Self::Vehicle { .. } => "vehicle",
            Self::Ped { .. } => "ped",
        }
    }
    pub fn refs(&self) -> Vec<&Blob> {
        match self {
            Self::Dlc { source, .. } | Self::Gameconfig { source } => vec![source],
            Self::Vehicle { files } | Self::Ped { files, .. } => {
                files.iter().map(|a| &a.source).collect()
            }
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Item {
    pub id: String,
    pub artifact: String,
    pub digest: String,
    pub menyoo: bool,
    pub contribution: Contribution,
}
impl Item {
    pub fn keys(&self) -> Vec<String> {
        let mut keys = vec!["update".into()];
        match &self.contribution {
            Contribution::Dlc { pack_name, .. } => keys.push(format!("dlc:{pack_name}")),
            Contribution::Vehicle { .. } | Contribution::Ped { .. } => keys.push("gmm".into()),
            _ => {}
        }
        if self.menyoo {
            keys.push("menyoo".into());
        }
        keys
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Binding {
    pub game: String,
    pub root: PathBuf,
    pub slug: String,
    pub version: String,
    pub exe: Fingerprint,
    pub original: Fingerprint,
}
#[derive(Clone, Serialize, Deserialize, Default)]
pub(super) struct State {
    pub binding: Option<Binding>,
    pub items: Vec<Item>,
    pub files: BTreeMap<String, Blob>,
    pub bases: BTreeMap<String, Option<Blob>>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct WireTarget {
    pub id: String,
    pub baseline: Option<Blob>,
    pub contributions: Vec<Contribution>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Input {
    pub schema_version: u8,
    pub job_id: String,
    pub kind: String,
    pub targets: Vec<WireTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_dlc: Option<Blob>,
}
#[derive(Serialize, Deserialize)]
pub(super) struct Frozen {
    // Creation-time identity is private. Never delete old jobs lacking it.
    #[serde(default)]
    pub workspace: Option<safe_fs::FileIdentity>,
    pub binding: Binding,
    pub parent: Option<String>,
    pub install: bool,
    pub item: Item,
    pub input: Input,
    pub bases: BTreeMap<String, Option<Blob>>,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Options {
    pack_name: Option<String>,
    model_name: Option<String>,
    sex: Option<String>,
    streamed: Option<bool>,
    menyoo: Option<bool>,
    source_dir: Option<String>,
}

pub(super) fn identifier(s: &str) -> bool {
    let first = s.bytes().next();
    let upper = s.to_ascii_uppercase();
    (1..=64).contains(&s.len())
        && first.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        && !matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit())
}
pub(super) fn destination(root: &Path, key: &str) -> AppResult<PathBuf> {
    Ok(match key {
        "update" => root.join("mods/update/update.rpf"),
        "gmm" => root.join("mods/update/x64/dlcpacks/gmm/dlc.rpf"),
        "menyoo" => root.join("menyooStuff/AddedVehicleModels.xml"),
        _ => {
            let p = key
                .strip_prefix("dlc:")
                .filter(|v| identifier(v) && !v.eq_ignore_ascii_case("gmm"))
                .ok_or_else(|| invalid("DLC 标识不在白名单内"))?;
            root.join("mods/update/x64/dlcpacks")
                .join(p)
                .join("dlc.rpf")
        }
    })
}
pub(super) fn output_name(id: &str) -> AppResult<&'static str> {
    match id {
        "update" => Ok("update.rpf"),
        "gmm" => Ok("gmm.rpf"),
        "menyoo" => Ok("menyoo.xml"),
        _ => Err(invalid("未知 RPF 产物")),
    }
}
/// A contribution is never allowed to silently override another Mod. In
/// particular the two GMM tables share the model-name namespace.
pub(super) fn unique(items: &[Item]) -> AppResult<()> {
    if items.len() > 256 {
        return Err(invalid("一个游戏的 RPF 贡献超过上限"));
    }
    let mut seen = BTreeSet::new();
    for i in items {
        let mut keys = BTreeSet::from([
            format!("artifact:{}", i.artifact),
            format!("hash:{}", i.digest),
        ]);
        match &i.contribution {
            Contribution::Dlc { pack_name, .. } => {
                keys.insert(format!("pack:{}", pack_name.to_ascii_lowercase()));
            }
            Contribution::Gameconfig { .. } => {
                keys.insert("singleton:gameconfig".into());
            }
            Contribution::Ped { model_name, .. } => {
                keys.insert(format!("model:{}", model_name.to_ascii_lowercase()));
            }
            Contribution::Vehicle { files } => {
                for f in files {
                    let stem = f.name.split('.').next().unwrap().to_ascii_lowercase();
                    keys.insert(format!(
                        "model:{}",
                        stem.strip_suffix("_hi").unwrap_or(&stem)
                    ));
                }
            }
        }
        if !keys.into_iter().all(|k| seen.insert(k)) {
            return Err(conflict(
                "RPF 原包、DLC 名称、模型名或 gameconfig 已被其他贡献使用",
            ));
        }
    }
    Ok(())
}
pub(super) fn after_items(prior: &State, item: &Item, install: bool) -> AppResult<Vec<Item>> {
    let mut items = prior.items.clone();
    if install {
        items.push(item.clone());
    } else {
        let n = items.len();
        items.retain(|x| x.id != item.id);
        if items.len() + 1 != n {
            return Err(conflict("该贡献不再处于活跃状态"));
        }
    }
    unique(&items)?;
    Ok(items)
}
fn asset_name(n: &str) -> bool {
    n.rsplit_once('.').is_some_and(|(stem, ext)| {
        identifier(stem) && matches!(ext, "yft" | "ytd" | "ydd" | "yld" | "ymt")
    })
}
/// Select from the native verified inventory only. sourceDir is a filter, not
/// a filesystem path. Variant collisions and multi-root bundles require review.
pub(super) fn make_item<F>(
    kind: &str,
    options: Option<&Value>,
    entry: &mod_library::LibraryEntry,
    members: &BTreeMap<String, PathBuf>,
    mut freeze: F,
) -> AppResult<Item>
where
    F: FnMut(&str, &Path) -> AppResult<Blob>,
{
    let o: Options = serde_json::from_value(options.cloned().unwrap_or_else(|| json!({})))?;
    if !matches!(kind, "dlc" | "gameconfig" | "ped" | "vehicle")
        || (kind != "dlc" && o.pack_name.is_some())
        || (kind != "ped" && (o.model_name.is_some() || o.sex.is_some() || o.streamed.is_some()))
        || (!matches!(kind, "vehicle" | "dlc") && o.menyoo.is_some())
    {
        return Err(invalid("RPF 类型和选项不匹配"));
    }
    let prefix = match o.source_dir {
        None => String::new(),
        Some(s) => {
            if s.len() > 512
                || s.split('/').any(|c| {
                    c.is_empty()
                        || matches!(c, "." | "..")
                        || c.contains(['\\', ':'])
                        || c.chars().any(char::is_control)
                })
            {
                return Err(invalid("sourceDir 必须是原包内的相对目录"));
            }
            format!("{s}/")
        }
    };
    let selected: Vec<_> = members
        .iter()
        .filter_map(|(k, p)| {
            let n = k.replace('\\', "/");
            n.strip_prefix(&prefix).map(|r| (k, p, r.to_lowercase()))
        })
        .collect();
    let contribution = if matches!(kind, "dlc" | "gameconfig") {
        let leaf = if kind == "dlc" {
            "dlc.rpf"
        } else {
            "gameconfig.xml"
        };
        let matching: Vec<_> = selected
            .iter()
            .filter(|(_, _, s)| s.rsplit('/').next() == Some(leaf))
            .collect();
        let [(key, path, _)] = matching.as_slice() else {
            return Err(invalid(
                "需要唯一匹配的 dlc.rpf 或 gameconfig.xml，请用 sourceDir 明确变体",
            ));
        };
        let source = freeze(key, path)?;
        if kind == "dlc" {
            let pack_name = match o.pack_name {
                Some(s) if identifier(&s) && !s.eq_ignore_ascii_case("gmm") => {
                    s.to_ascii_lowercase()
                }
                None => String::new(),
                _ => return Err(invalid("DLC packName 无效或为保留名称")),
            };
            Contribution::Dlc { pack_name, source }
        } else {
            Contribution::Gameconfig { source }
        }
    } else {
        let ped = kind == "ped";
        let model = o.model_name.unwrap_or_default().to_ascii_lowercase();
        let streamed = o.streamed.unwrap_or(false);
        if ped
            && (!identifier(&model)
                || !matches!(o.sex.as_deref(), Some("Male" | "Female"))
                || o.streamed.is_none())
        {
            return Err(invalid("Ped 需要有效 modelName、sex 和 streamed"));
        }
        let mut files = Vec::new();
        let mut names = BTreeSet::new();
        let mut dirs = BTreeSet::new();
        let mut component = false;
        for (key, p, relative) in selected {
            let (dir, leaf) = relative.rsplit_once('/').unwrap_or(("", &relative));
            let (stem, ext) = leaf.rsplit_once('.').unwrap_or((leaf, ""));
            if !matches!(ext, "ydd" | "yft" | "yld" | "ymt" | "ytd") {
                continue;
            }
            if !ped && !matches!(ext, "yft" | "ytd") {
                return Err(invalid("载具包混有 Ped 资源，请明确 sourceDir"));
            }
            let (base, name) = if ped && streamed && dir.rsplit('/').next() == Some(model.as_str())
            {
                component = true;
                (
                    dir.rsplit_once('/').map_or("", |(p, _)| p),
                    format!("{model}/{leaf}"),
                )
            } else if !ped || stem == model {
                (dir, leaf.to_owned())
            } else {
                continue;
            };
            if !asset_name(leaf) || !names.insert(name.clone()) || files.len() >= 512 {
                return Err(invalid("模型文件名无效、重复或数量过多"));
            }
            dirs.insert(base.to_owned());
            files.push(Asset {
                name,
                source: freeze(key, p)?,
            });
        }
        if dirs.len() != 1 || files.is_empty() || ped && streamed && !component {
            return Err(invalid("未选中唯一模型目录或未找到 Streamed Ped 组件"));
        }
        if ped {
            if !files.iter().any(|a| {
                a.name == format!("{model}.ymt")
                    || a.name == format!("{model}.yft")
                    || (!streamed && a.name == format!("{model}.ydd"))
            }) {
                return Err(invalid("缺少与 modelName 匹配的 Ped 主模型"));
            }
            Contribution::Ped {
                model_name: model,
                sex: o.sex.unwrap(),
                streamed,
                files,
            }
        } else {
            if !files
                .iter()
                .any(|a| a.name.ends_with(".yft") && !a.name.ends_with("_hi.yft"))
            {
                return Err(invalid("载具缺少主 YFT 模型"));
            }
            Contribution::Vehicle { files }
        }
    };
    Ok(Item {
        id: util::new_id("install"),
        artifact: entry.id.clone(),
        digest: entry.sha256.clone(),
        menyoo: o.menyoo.unwrap_or(false),
        contribution,
    })
}
