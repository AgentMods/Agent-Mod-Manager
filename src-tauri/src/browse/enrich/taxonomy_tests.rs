use super::{fallback_tags, fallback_type, ModType};

#[test]
fn legacy_and_new_type_keys_round_trip() {
    for (key, kind) in [
        ("visual", ModType::Visual),
        ("localization", ModType::Localization),
        ("gameplay", ModType::Gameplay),
        ("audio", ModType::Audio),
        ("framework", ModType::Framework),
        ("other", ModType::Other),
        ("unknown", ModType::Unknown),
        ("tools", ModType::Tools),
        ("fixes", ModType::Fixes),
        ("interface", ModType::Interface),
        ("vehicles", ModType::Vehicles),
        ("characters", ModType::Characters),
        ("performance", ModType::Performance),
    ] {
        let wire_value = serde_json::json!(key);
        assert_eq!(kind.as_str(), key);
        assert_eq!(serde_json::to_value(kind).unwrap(), wire_value);
        assert_eq!(serde_json::from_value::<ModType>(wire_value).unwrap(), kind);
    }
}

#[test]
fn recognizes_source_categories_and_tags() {
    for (label, expected) in [
        ("Visuals and Graphics", ModType::Visual),
        ("Visuals & Graphics", ModType::Visual),
        ("本地化", ModType::Localization),
        ("Gameplay Effects and Changes", ModType::Gameplay),
        ("音效", ModType::Audio),
        ("Mod Loaders", ModType::Framework),
        ("Tools & Utilities", ModType::Tools),
        ("工具/修改器", ModType::Tools),
        ("Installers", ModType::Tools),
        ("Compatibility Patches", ModType::Fixes),
        ("Bug Fixes & Compatibility", ModType::Fixes),
        ("修复与兼容", ModType::Fixes),
        ("UI / HUD", ModType::Interface),
        ("Vehicles", ModType::Vehicles),
        ("载具", ModType::Vehicles),
        ("Clothing and Accessories", ModType::Characters),
        ("角色与外观", ModType::Characters),
        ("Characters & Appearance", ModType::Characters),
        ("Performance Optimization", ModType::Performance),
        ("性能优化", ModType::Performance),
    ] {
        let tags = fallback_tags(&[label.into()]);
        assert_eq!(tags[0].label, label);
        assert_eq!(
            fallback_type(Some(label), &[], "Example", None),
            expected,
            "category {label:?}"
        );
        assert_eq!(
            fallback_type(Some("mod"), &tags, "Example", None),
            expected,
            "tag {label:?}"
        );
    }
}

#[test]
fn generic_categories_leave_bilingual_title_signals_available() {
    for category in [
        None,
        Some("mod"),
        Some("Other"),
        Some("Miscellaneous"),
        Some("其他"),
    ] {
        for (title, expected) in [
            ("GTA5增强版截图修改器", ModType::Tools),
            ("Photo-Mode Enhancer", ModType::Tools),
            ("Example Installer v2", ModType::Tools),
            ("Unofficial Fixes", ModType::Fixes),
            ("镜头异常修复", ModType::Fixes),
            ("Gauges", ModType::Interface),
            ("新版仪表盘", ModType::Interface),
            ("游戏DLSS补丁", ModType::Performance),
            ("赛车载具包", ModType::Vehicles),
            ("Chinese New Year Outfit", ModType::Characters),
            ("Textures — DLSS Frame Generation", ModType::Performance),
            ("HUD Fix 简体中文汉化补丁", ModType::Localization),
            ("Chinese Translation – HUD Fix", ModType::Localization),
        ] {
            assert_eq!(
                fallback_type(category, &[], title, None),
                expected,
                "category {category:?}, title {title:?}"
            );
        }
    }
}

#[test]
fn uses_specific_bilingual_summary_when_title_and_category_are_generic() {
    for (summary, expected) in [
        (
            "A standalone installer for the community mod pack.",
            ModType::Tools,
        ),
        ("这是一款用于截图的工具。", ModType::Tools),
        ("Fixes startup crashes in the campaign.", ModType::Fixes),
        ("修复了开场动画闪退问题。", ModType::Fixes),
        (
            "Redesigns the gauges for the player HUD.",
            ModType::Interface,
        ),
        ("为玩家添加仪表盘与计时器。", ModType::Interface),
        ("Adds new vehicles to the campaign.", ModType::Vehicles),
        ("为主角添加新服装。", ModType::Characters),
        (
            "Adds support for DLSS and improves frame rates.",
            ModType::Performance,
        ),
        ("减少卡顿并提升帧率。", ModType::Performance),
        ("为游戏添加DLSS支持。", ModType::Performance),
        (
            "Provides a Chinese translation of the base game.",
            ModType::Localization,
        ),
        ("为原版游戏提供汉化。", ModType::Localization),
    ] {
        assert_eq!(
            fallback_type(Some("其他"), &[], "Example", Some(summary)),
            expected,
            "summary {summary:?}"
        );
    }
}

#[test]
fn explicit_category_and_clear_title_take_precedence_over_weaker_signals() {
    let tags = fallback_tags(&["Textures".into(), "Music".into()]);
    assert_eq!(
        fallback_type(Some("Tools"), &tags, "HUD", None),
        ModType::Tools
    );
    assert_eq!(
        fallback_type(Some("mod"), &tags, "HUD", None),
        ModType::Interface
    );
    assert_eq!(
        fallback_type(
            Some("mod"),
            &tags,
            "Example",
            Some("Adds a new quest to the campaign.")
        ),
        ModType::Gameplay
    );
    assert_eq!(fallback_type(None, &tags, "Example", None), ModType::Other);
}

#[test]
fn incidental_words_and_generic_metadata_do_not_create_a_type() {
    for (title, summary) in [
        ("Mod", "A small pack for the base game."),
        ("Other", ""),
        ("Unknown", ""),
        (
            "For the Community",
            "Use your preferred installer, or see the installation guide.",
        ),
        (
            "Build on the Coast",
            "Screenshots taken using the photo mode. Compatible with DLSS and other mods.",
        ),
        (
            "Gaugestone UninstallerPlus Prefixes",
            "This is a fixed-camera sequence with three new frames.",
        ),
        (
            "Chinese New Year",
            "The story is set in China. Chinese translation is available separately.",
        ),
        ("全新模组", "需要安装修改器。截图请参考说明。"),
        ("DLSSifier", "A small pack for the base game."),
        ("Example", ""),
    ] {
        assert_eq!(
            fallback_type(Some("Mod"), &[], title, Some(summary)),
            ModType::Other,
            "title {title:?}, summary {summary:?}"
        );
    }
}
