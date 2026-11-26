use super::{
    clip, BrowseTag, MAX_KIND_BYTES, MAX_RAW_TAGS, MAX_SUMMARY_BYTES, MAX_TAGS, MAX_TAG_BYTES,
    MAX_TITLE_BYTES,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// Cache keys include this epoch as well as the exact prompt. Bump it when
// alias meaning, input projection, validation or rank semantics change.
pub(super) const VERSION: &str = "browse-taxonomy-v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ModType {
    Visual,
    Localization,
    Gameplay,
    Audio,
    Framework,
    Tools,
    Fixes,
    Interface,
    Vehicles,
    Characters,
    Performance,
    Other,
    Unknown,
}

impl ModType {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Visual => "visual",
            Self::Localization => "localization",
            Self::Gameplay => "gameplay",
            Self::Audio => "audio",
            Self::Framework => "framework",
            Self::Tools => "tools",
            Self::Fixes => "fixes",
            Self::Interface => "interface",
            Self::Vehicles => "vehicles",
            Self::Characters => "characters",
            Self::Performance => "performance",
            Self::Other => "other",
            Self::Unknown => "unknown",
        }
    }
}

impl Serialize for ModType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

fn normalized(label: &str) -> String {
    let mut key = String::new();
    let mut separator = false;
    for ch in label.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            if separator {
                key.push(' ');
            }
            key.push(ch);
            separator = false;
        } else {
            separator = !key.is_empty();
        }
    }
    key
}

fn known_tag(normalized: &str) -> Option<&'static str> {
    Some(match normalized {
        "accessibility" | "barrierefreiheit" | "accessibilité" | "无障碍" => "accessibility",
        "animation" | "animations" | "animación" | "动画" | "アニメーション" => {
            "animation"
        }
        "armor" | "armour" | "armors" | "armour and shields" | "armure" | "armaduras"
        | "rüstung" | "护甲" | "防具" | "装甲" => "armor",
        "audio" | "sound" | "sounds" | "sound effects" | "sound fx" | "son" | "声音" | "音效"
        | "オーディオ" | "사운드" | "звук" => "audio",
        "balance" | "balancing" | "équilibrage" | "平衡" => "balance",
        "building" | "buildings" | "construction" | "建筑" | "建造" => "building",
        "characters"
        | "character"
        | "character presets"
        | "character customization"
        | "character appearance"
        | "characters and appearance"
        | "characters appearance"
        | "personnages"
        | "角色"
        | "人物"
        | "角色与外观"
        | "キャラクター" => "characters",
        "cheats" | "cheat" | "cheats and god items" | "作弊" => "cheats",
        "compatibility" | "compatibility patches" | "kompatibilität" | "兼容" | "兼容性" => {
            "compatibility"
        }
        "crafting" | "crafts" | "fabrication" | "合成" | "制作" => "crafting",
        "fixes"
        | "fix"
        | "bug fix"
        | "bugfix"
        | "bug fixes"
        | "bugfixes"
        | "patches and fixes"
        | "fixes and compatibility"
        | "fixes compatibility"
        | "bug fixes and compatibility"
        | "bug fixes compatibility"
        | "修复"
        | "修复补丁"
        | "修复与兼容"
        | "バグ修正" => "fixes",
        "framework"
        | "frameworks"
        | "api"
        | "library"
        | "libraries"
        | "mod loader"
        | "mod loaders"
        | "modding framework"
        | "框架"
        | "前置"
        | "ライブラリ"
        | "библиотеки" => "framework",
        "graphics"
        | "visuals"
        | "visual"
        | "visuals and graphics"
        | "visuals graphics"
        | "grafik"
        | "gráficos"
        | "画质"
        | "视觉"
        | "グラフィックス" => "graphics",
        "interface" | "user interface" | "gui" | "hud" | "ui" | "ui hud" | "界面" | "用户界面" => {
            "interface"
        }
        "lighting" | "lights" | "beleuchtung" | "光照" | "光影" => "lighting",
        "maps" | "map" | "levels" | "地图" | "關卡" | "关卡" | "マップ" | "карты" => {
            "maps"
        }
        "models" | "model" | "3d models" | "模型" | "3dモデル" => "models",
        "movement" | "movements" | "移动" | "移动机制" => "movement",
        "multiplayer" | "multi player" | "co op" | "coop" | "联机" | "多人" => "multiplayer",
        "music" | "soundtrack" | "musique" | "musik" | "música" | "音乐" | "音楽" | "음악"
        | "музыка" => "music",
        "optimization"
        | "optimisation"
        | "performance"
        | "performance boost"
        | "performance optimization"
        | "performance optimisation"
        | "优化"
        | "性能优化"
        | "パフォーマンス" => "optimization",
        "quests" | "quest" | "missions" | "quêtes" | "任务" | "クエスト" => "quests",
        "qol" | "quality of life" | "游戏体验" | "易用性" | "便捷功能" => "qol",
        "shaders" | "shader" | "shader packs" | "reshade" | "着色器" | "シェーダー" => {
            "shaders"
        }
        "textures"
        | "texture"
        | "texture packs"
        | "texturepack"
        | "texture packs and textures"
        | "texturen"
        | "texturas"
        | "材质"
        | "材质包"
        | "贴图"
        | "紋理"
        | "纹理"
        | "テクスチャ"
        | "텍스처"
        | "текстуры" => "textures",
        "tools"
        | "tool"
        | "utilities"
        | "tools and utilities"
        | "tools utilities"
        | "utilities and tools"
        | "utilities tools"
        | "tools trainers"
        | "tools and trainers"
        | "outils"
        | "werkzeuge"
        | "herramientas"
        | "工具"
        | "工具 修改器"
        | "工具与修改器"
        | "ツール" => "tools",
        "translation" | "translations" | "localization" | "localisation" | "language pack"
        | "language packs" | "übersetzung" | "traduction" | "traducción" | "tradução"
        | "traduzione" | "翻译" | "翻譯" | "汉化" | "漢化" | "汉化补丁" | "本地化" | "日本語化"
        | "翻訳" | "번역" | "перевод" => "translation",
        "vehicles" | "vehicle" | "cars" | "véhicules" | "fahrzeuge" | "车辆" | "载具" => {
            "vehicles"
        }
        "weapons" | "weapon" | "waffen" | "armes" | "armas" | "武器" => "weapons",
        "world" | "world generation" | "worldgen" | "地形生成" | "世界生成" => "world",
        _ => return None,
    })
}

pub(super) fn is_stable_tag(key: &str) -> bool {
    known_tag(key) == Some(key)
}

pub(super) fn fallback_tags(tags: &[String]) -> Vec<BrowseTag> {
    let mut result: Vec<BrowseTag> = Vec::new();
    for tag in tags.iter().take(MAX_RAW_TAGS) {
        if tag.len() > MAX_TAG_BYTES || tag.chars().any(char::is_control) {
            continue;
        }
        let label = tag.trim();
        let normalized = normalized(label);
        if normalized.is_empty() {
            continue;
        }
        let key = if let Some(known) = known_tag(&normalized) {
            known.to_owned()
        } else {
            // Preserve provider-specific labels without pretending that an
            // unseen spelling has a known cross-language semantic equivalent.
            let digest = Sha256::digest(normalized.as_bytes());
            format!("raw-{}", hex::encode(&digest[..12]))
        };
        if !result.iter().any(|tag| tag.key == key) {
            result.push(BrowseTag {
                key,
                label: label.to_owned(),
            });
            if result.len() >= MAX_TAGS {
                break;
            }
        }
    }
    result
}

fn type_for_tag(key: &str) -> Option<ModType> {
    Some(match key {
        "graphics" | "lighting" | "models" | "shaders" | "textures" => ModType::Visual,
        "translation" => ModType::Localization,
        "balance" | "cheats" | "crafting" | "movement" | "quests" | "world" => ModType::Gameplay,
        "audio" | "music" => ModType::Audio,
        "framework" => ModType::Framework,
        "tools" => ModType::Tools,
        "fixes" | "compatibility" => ModType::Fixes,
        "interface" => ModType::Interface,
        "vehicles" => ModType::Vehicles,
        "characters" => ModType::Characters,
        "optimization" => ModType::Performance,
        _ => return None,
    })
}

fn type_for_kind(kind: &str) -> Option<ModType> {
    if kind.len() > MAX_KIND_BYTES {
        return None;
    }
    let kind = normalized(kind);
    if let Some(known) = known_tag(&kind).and_then(type_for_tag) {
        return Some(known);
    }
    Some(match kind.as_str() {
        "appearance"
        | "visual enhancement"
        | "visual enhancements"
        | "models and textures"
        | "cosmetics"
        | "外观"
        | "视觉增强"
        | "美化" => ModType::Visual,
        "gameplay"
        | "gameplay effects and changes"
        | "gameplay changes"
        | "gameplay tweaks"
        | "new content"
        | "overhauls"
        | "玩法"
        | "玩法增强"
        | "游戏性"
        | "ゲームプレイ" => ModType::Gameplay,
        "trainer" | "trainers" | "installer" | "installers" | "mod installer" | "修改器"
        | "安装器" | "安装工具" => ModType::Tools,
        "gauges" => ModType::Interface,
        "automobiles" | "汽车" => ModType::Vehicles,
        "skins"
        | "outfits"
        | "clothing"
        | "clothing and accessories"
        | "皮肤"
        | "服装"
        | "捏脸" => ModType::Characters,
        "dlss" | "frame generation" => ModType::Performance,
        _ => return None,
    })
}

// Summary phrases include context because descriptions also mention requirements,
// installation and screenshots. Rule order resolves titles with several signals.
struct TextRule {
    type_key: ModType,
    title: &'static [&'static str],
    summary: &'static [&'static str],
}

const TEXT_RULES: &[TextRule] = &[
    TextRule {
        type_key: ModType::Localization,
        title: &[
            "chinese translation",
            "english translation",
            "fan translation",
            "translation patch",
            "language pack",
            "localization patch",
            "汉化",
            "漢化",
            "中文补丁",
            "中文翻译",
        ],
        summary: &[
            "adds chinese translation",
            "adds a chinese translation",
            "translates the mod",
            "translates the game",
            "provides a chinese translation",
            "chinese language pack",
            "提供汉化",
            "完整汉化",
            "简体中文翻译",
            "翻译为简体中文",
            "中文补丁",
        ],
    },
    TextRule {
        type_key: ModType::Fixes,
        title: &[
            "fix",
            "fixes",
            "bugfix",
            "bugfixes",
            "compatibility patch",
            "修复",
            "修復",
            "兼容补丁",
            "兼容性补丁",
        ],
        summary: &[
            "fixes bugs",
            "fixes game bugs",
            "fixes various bugs",
            "fixes crashes",
            "fixes startup crashes",
            "resolves crashes",
            "fixes an issue",
            "fixes a bug",
            "compatibility patch for",
            "修复了",
            "修复游戏",
            "修复崩溃",
            "修复闪退",
            "解决崩溃",
            "兼容性补丁",
        ],
    },
    TextRule {
        type_key: ModType::Tools,
        title: &[
            "trainer",
            "trainers",
            "installer",
            "uninstaller",
            "mod manager",
            "modding tool",
            "screenshot tool",
            "screenshot utility",
            "screenshot editor",
            "photo mode tool",
            "photo mode enhancer",
            "photo mode unlocker",
            "修改器",
            "安装器",
            "安装工具",
            "截图工具",
            "模组管理器",
        ],
        summary: &[
            "a tool for taking screenshots",
            "a screenshot utility",
            "a standalone installer",
            "an installer for",
            "a tool for installing mods",
            "a utility for installing mods",
            "standalone mod manager",
            "a trainer for",
            "截图修改器",
            "用于截图的工具",
            "自动安装模组的工具",
            "模组安装器",
        ],
    },
    TextRule {
        type_key: ModType::Performance,
        title: &[
            "dlss",
            "fps boost",
            "frame generation",
            "performance boost",
            "performance optimization",
            "performance optimisation",
            "性能优化",
            "帧数优化",
            "帧率优化",
            "帧生成",
        ],
        summary: &[
            "adds dlss",
            "enables dlss",
            "adds support for dlss",
            "replaces fsr with dlss",
            "adds frame generation",
            "improves frame rates",
            "boosts fps",
            "reduces stuttering",
            "启用dlss",
            "添加dlss支持",
            "提升帧率",
            "提高帧率",
            "提升帧数",
            "减少卡顿",
            "优化游戏性能",
        ],
    },
    TextRule {
        type_key: ModType::Framework,
        title: &[
            "mod loader",
            "modding framework",
            "script extender",
            "前置框架",
            "模组加载器",
        ],
        summary: &[
            "a framework for mod developers",
            "a library for mod developers",
            "provides a modding api",
            "用于开发模组的框架",
            "提供模组开发接口",
        ],
    },
    TextRule {
        type_key: ModType::Audio,
        title: &[
            "soundtrack",
            "sound effects",
            "sound replacer",
            "音效",
            "配乐",
        ],
        summary: &[
            "replaces the soundtrack",
            "adds new sound effects",
            "替换游戏配乐",
            "替换音效",
        ],
    },
    TextRule {
        type_key: ModType::Interface,
        title: &["gauges", "hud", "ui", "user interface", "界面", "仪表盘"],
        summary: &[
            "adds gauges",
            "replaces gauges",
            "redesigns the gauges",
            "replaces the hud",
            "redesigns the hud",
            "customizes the user interface",
            "自定义界面",
            "重制界面",
            "添加仪表盘",
        ],
    },
    TextRule {
        type_key: ModType::Vehicles,
        title: &[
            "vehicle pack",
            "car pack",
            "vehicle replacement",
            "载具包",
            "载具替换",
            "车辆替换",
        ],
        summary: &[
            "adds new vehicles",
            "adds new cars",
            "replaces a vehicle",
            "添加新载具",
            "添加新车辆",
        ],
    },
    TextRule {
        type_key: ModType::Characters,
        title: &[
            "outfit",
            "outfits",
            "character preset",
            "character presets",
            "character customization",
            "hair pack",
            "服装",
            "捏脸",
            "角色外观",
            "角色皮肤",
            "时装",
        ],
        summary: &[
            "adds new outfits",
            "adds a new outfit",
            "replaces the player model",
            "添加新服装",
            "替换角色外观",
            "替换角色模型",
        ],
    },
    TextRule {
        type_key: ModType::Visual,
        title: &[
            "texture pack",
            "textures",
            "reshade",
            "lighting overhaul",
            "材质包",
            "高清贴图",
            "光影",
        ],
        summary: &[
            "replaces textures",
            "overhauls the lighting",
            "替换游戏贴图",
            "重制光照",
        ],
    },
    TextRule {
        type_key: ModType::Gameplay,
        title: &[
            "gameplay overhaul",
            "combat overhaul",
            "quest pack",
            "玩法增强",
            "战斗重制",
            "任务包",
        ],
        summary: &[
            "adds new quests",
            "adds a new quest",
            "overhauls combat",
            "新增任务",
            "重制战斗",
        ],
    },
];

fn contains_phrase(text: &str, phrase: &str) -> bool {
    if !phrase.is_ascii() {
        return text.contains(phrase);
    }
    // English terms in Chinese titles can meet Han characters without spaces.
    let boundary = |ch| matches!(ch, ' ' | '\u{3400}'..='\u{9fff}');
    text.match_indices(phrase).any(|(start, matched)| {
        let end = start + matched.len();
        text[..start].chars().next_back().is_none_or(boundary)
            && text[end..].chars().next().is_none_or(boundary)
    })
}

fn type_for_text(text: &str, is_title: bool) -> Option<ModType> {
    let text = normalized(text);
    TEXT_RULES.iter().find_map(|rule| {
        let phrases = if is_title { rule.title } else { rule.summary };
        phrases
            .iter()
            .any(|phrase| contains_phrase(&text, phrase))
            .then_some(rule.type_key)
    })
}

fn consensus_type_for_tags(tags: &[BrowseTag]) -> Option<ModType> {
    let mut candidates = tags
        .iter()
        .filter_map(|tag| type_for_tag(&tag.key).or_else(|| type_for_kind(&tag.label)));
    let first = candidates.next()?;
    candidates
        .all(|candidate| candidate == first)
        .then_some(first)
}

pub(super) fn fallback_type(
    kind: Option<&str>,
    tags: &[BrowseTag],
    title: &str,
    summary: Option<&str>,
) -> ModType {
    kind.and_then(type_for_kind)
        .or_else(|| type_for_text(clip(title, MAX_TITLE_BYTES), true))
        .or_else(|| type_for_kind(title))
        .or_else(|| consensus_type_for_tags(tags))
        .or_else(|| summary.and_then(|text| type_for_text(clip(text, MAX_SUMMARY_BYTES), false)))
        .unwrap_or(ModType::Other)
}

#[cfg(test)]
#[path = "taxonomy_tests.rs"]
mod tests;
