use super::{clip, BrowseTag, MAX_KIND_BYTES, MAX_RAW_TAGS, MAX_TAGS, MAX_TAG_BYTES};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// Cache keys include this epoch as well as the exact prompt. Bump it when
// alias meaning, input projection, validation or rank semantics change.
pub(super) const VERSION: &str = "browse-taxonomy-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ModType {
    Visual,
    Localization,
    Gameplay,
    Audio,
    Framework,
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
        "characters" | "character" | "character presets" | "personnages" | "角色" | "人物"
        | "キャラクター" => "characters",
        "cheats" | "cheat" | "cheats and god items" | "作弊" => "cheats",
        "compatibility" | "compatibility patches" | "kompatibilität" | "兼容" | "兼容性" => {
            "compatibility"
        }
        "crafting" | "crafts" | "fabrication" | "合成" | "制作" => "crafting",
        "fixes" | "bugfix" | "bug fixes" | "bugfixes" | "patches and fixes" | "修复"
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
        | "grafik"
        | "gráficos"
        | "画质"
        | "视觉"
        | "グラフィックス" => "graphics",
        "interface" | "user interface" | "gui" | "hud" | "ui" | "界面" | "用户界面" => {
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
        "tools" | "tool" | "utilities" | "outils" | "werkzeuge" | "herramientas" | "工具"
        | "ツール" => "tools",
        "translation" | "translations" | "localization" | "localisation" | "language pack"
        | "language packs" | "übersetzung" | "traduction" | "traducción" | "tradução"
        | "traduzione" | "翻译" | "翻譯" | "汉化" | "本地化" | "日本語化" | "翻訳" | "번역"
        | "перевод" => "translation",
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
        _ => return None,
    })
}

fn type_for_kind(kind: &str) -> Option<ModType> {
    if kind.len() > MAX_KIND_BYTES {
        return None;
    }
    let kind = normalized(clip(kind, MAX_KIND_BYTES));
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
        "other" | "misc" | "miscellaneous" | "divers" | "sonstiges" | "其他" | "其它" => {
            ModType::Other
        }
        _ => return None,
    })
}

pub(super) fn fallback_type(kind: Option<&str>, tags: &[BrowseTag]) -> ModType {
    if let Some(explicit) = kind
        .and_then(type_for_kind)
        .filter(|kind| *kind != ModType::Other)
    {
        return explicit;
    }
    let mut candidates = tags.iter().filter_map(|tag| type_for_tag(&tag.key));
    match candidates.next() {
        Some(first) if candidates.all(|candidate| candidate == first) => first,
        Some(_) => ModType::Unknown,
        None => kind.and_then(type_for_kind).unwrap_or(ModType::Unknown),
    }
}
