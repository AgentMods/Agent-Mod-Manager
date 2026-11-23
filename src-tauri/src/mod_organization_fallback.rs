//! Conservative local fallback for installs prepared without model suggestions.
//! Keep proper names intact. Translate only complete, recognizable category or
//! descriptor phrases, optionally following a title separated by a delimiter.

struct Vocabulary {
    language: &'static str,
    categories: [&'static str; 6],
    descriptors: [&'static str; 6],
}

const WORDS: &[Vocabulary] = &[
    Vocabulary {
        language: "en",
        categories: [
            "Visuals",
            "Localization",
            "Gameplay",
            "Audio",
            "Framework",
            "Other",
        ],
        descriptors: [
            "Texture Pack",
            "Character Replacement",
            "Sound Overhaul",
            "Performance Fix",
            "UI Overhaul",
            "Weapon Pack",
        ],
    },
    Vocabulary {
        language: "zh-Hans",
        categories: ["视觉增强", "本地化", "玩法", "音频", "框架", "其他"],
        descriptors: [
            "材质包",
            "角色替换",
            "音效重制",
            "性能修复",
            "界面重制",
            "武器包",
        ],
    },
    Vocabulary {
        language: "zh-Hant",
        categories: ["視覺增強", "在地化", "玩法", "音效", "框架", "其他"],
        descriptors: [
            "材質包",
            "角色替換",
            "音效重製",
            "效能修正",
            "介面重製",
            "武器包",
        ],
    },
    Vocabulary {
        language: "ja",
        categories: [
            "ビジュアル",
            "ローカライズ",
            "ゲームプレイ",
            "サウンド",
            "フレームワーク",
            "その他",
        ],
        descriptors: [
            "テクスチャパック",
            "キャラクター差し替え",
            "サウンド刷新",
            "パフォーマンス修正",
            "UI刷新",
            "武器パック",
        ],
    },
    Vocabulary {
        language: "ko",
        categories: [
            "그래픽",
            "현지화",
            "게임플레이",
            "오디오",
            "프레임워크",
            "기타",
        ],
        descriptors: [
            "텍스처 팩",
            "캐릭터 교체",
            "음향 개편",
            "성능 수정",
            "UI 개편",
            "무기 팩",
        ],
    },
    Vocabulary {
        language: "fr",
        categories: [
            "Graphismes",
            "Localisation",
            "Jouabilité",
            "Audio",
            "Framework",
            "Autres",
        ],
        descriptors: [
            "Pack de textures",
            "Remplacement de personnage",
            "Refonte audio",
            "Correctif de performances",
            "Refonte de l’interface",
            "Pack d’armes",
        ],
    },
    Vocabulary {
        language: "de",
        categories: [
            "Grafik",
            "Lokalisierung",
            "Spielmechanik",
            "Audio",
            "Framework",
            "Sonstiges",
        ],
        descriptors: [
            "Texturpaket",
            "Figurenersatz",
            "Audioüberarbeitung",
            "Leistungskorrektur",
            "UI-Überarbeitung",
            "Waffenpaket",
        ],
    },
    Vocabulary {
        language: "es",
        categories: [
            "Gráficos",
            "Localización",
            "Jugabilidad",
            "Audio",
            "Framework",
            "Otros",
        ],
        descriptors: [
            "Paquete de texturas",
            "Reemplazo de personaje",
            "Revisión de sonido",
            "Corrección de rendimiento",
            "Rediseño de interfaz",
            "Paquete de armas",
        ],
    },
    Vocabulary {
        language: "pt",
        categories: [
            "Gráficos",
            "Localização",
            "Jogabilidade",
            "Áudio",
            "Framework",
            "Outros",
        ],
        descriptors: [
            "Pacote de texturas",
            "Substituição de personagem",
            "Reformulação de áudio",
            "Correção de desempenho",
            "Reformulação de interface",
            "Pacote de armas",
        ],
    },
    Vocabulary {
        language: "it",
        categories: [
            "Grafica",
            "Localizzazione",
            "Giocabilità",
            "Audio",
            "Framework",
            "Altro",
        ],
        descriptors: [
            "Pacchetto texture",
            "Sostituzione personaggio",
            "Revisione audio",
            "Correzione prestazioni",
            "Revisione interfaccia",
            "Pacchetto armi",
        ],
    },
    Vocabulary {
        language: "ru",
        categories: [
            "Графика",
            "Локализация",
            "Игровой процесс",
            "Звук",
            "Фреймворк",
            "Прочее",
        ],
        descriptors: [
            "Пакет текстур",
            "Замена персонажа",
            "Переработка звука",
            "Исправление производительности",
            "Переработка интерфейса",
            "Набор оружия",
        ],
    },
    Vocabulary {
        language: "pl",
        categories: [
            "Grafika",
            "Lokalizacja",
            "Rozgrywka",
            "Dźwięk",
            "Framework",
            "Inne",
        ],
        descriptors: [
            "Pakiet tekstur",
            "Zamiana postaci",
            "Przebudowa dźwięku",
            "Poprawka wydajności",
            "Przebudowa interfejsu",
            "Pakiet broni",
        ],
    },
    Vocabulary {
        language: "uk",
        categories: [
            "Графіка",
            "Локалізація",
            "Ігровий процес",
            "Звук",
            "Фреймворк",
            "Інше",
        ],
        descriptors: [
            "Пакет текстур",
            "Заміна персонажа",
            "Переробка звуку",
            "Виправлення продуктивності",
            "Переробка інтерфейсу",
            "Набір зброї",
        ],
    },
    Vocabulary {
        language: "tr",
        categories: [
            "Grafikler",
            "Yerelleştirme",
            "Oynanış",
            "Ses",
            "Altyapı",
            "Diğer",
        ],
        descriptors: [
            "Doku paketi",
            "Karakter değişimi",
            "Ses yenilemesi",
            "Performans düzeltmesi",
            "Arayüz yenilemesi",
            "Silah paketi",
        ],
    },
];

fn vocabulary(locale: &str) -> Option<&'static Vocabulary> {
    let mut parts = locale.split('-');
    let language = parts.next()?;
    let language = if language.eq_ignore_ascii_case("zh") {
        let subtags: Vec<_> = parts.map(str::to_ascii_lowercase).collect();
        let hant = subtags.iter().any(|tag| tag == "hant")
            || (!subtags.iter().any(|tag| tag == "hans")
                && subtags
                    .iter()
                    .any(|tag| matches!(tag.as_str(), "tw" | "hk" | "mo")));
        if hant {
            "zh-Hant"
        } else {
            "zh-Hans"
        }
    } else {
        language
    };
    WORDS
        .iter()
        .find(|words| words.language.eq_ignore_ascii_case(language))
}

pub(crate) fn tag_name(category: &str, locale: &str) -> &'static str {
    let words = vocabulary(locale).unwrap_or(&WORDS[0]);
    let index = WORDS[0]
        .categories
        .iter()
        .position(|&key| key == category)
        .unwrap_or(5);
    words.categories[index]
}

pub(crate) fn display_name(original: &str, locale: &str) -> Option<String> {
    let target = vocabulary(locale)?;
    let title = original
        .rsplit_once('.')
        .filter(|(_, ext)| {
            ["zip", "7z", "rar", "jar", "pak"]
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
        .map_or(original, |(stem, _)| stem);
    let (prefix, phrase) = title
        .rsplit_once(" - ")
        .map_or((None, title), |(prefix, phrase)| (Some(prefix), phrase));
    let needle = phrase.trim().replace(['_', '-'], " ").to_lowercase();
    let translation = WORDS.iter().find_map(|words| {
        words
            .descriptors
            .iter()
            .position(|word| word.to_lowercase() == needle)
            .map(|index| target.descriptors[index])
    })?;
    Some(match prefix {
        Some(name) => format!("{name} - {translation}"),
        None => translation.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_complete_descriptors_and_preserves_proper_names() {
        assert_eq!(
            display_name("Sekiro - Texture Pack", "zh-TW").as_deref(),
            Some("Sekiro - 材質包")
        );
        assert_eq!(
            display_name("材质包", "de-AT").as_deref(),
            Some("Texturpaket")
        );
        assert_eq!(
            display_name("Texture-Pack.zip", "zh-CN").as_deref(),
            Some("材质包")
        );
        assert_eq!(display_name("Sekiro", "zh-CN"), None);
        assert_eq!(display_name("Texture Packer", "zh-CN"), None);
        assert_eq!(display_name("Texture Pack", "sv"), None);
        assert_eq!(tag_name("Audio", "ja-JP"), "サウンド");
        assert_eq!(tag_name("Visuals", "zh-Hans-HK"), "视觉增强");
    }
}
