use super::*;
use crate::{
    db::Db,
    plugins::test_fixtures::PACKAGES,
    settings::{
        AgentModelSettings, AppSettings, ComposerPreferences, ReasoningEffort, SecretStore,
    },
};
use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode},
    routing::post,
    Router,
};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

fn reply() -> Value {
    json!({"title":"Stardew Valley 模组适配", "summary":"SMAPI：安全管理模组。", "description":"识别游戏并核对模组类型。\n\n安装前可检查依赖和存档。", "tags":["SMAPI","模组管理"]})
}
fn fixture() -> (GamePackage, [PublicationFile; 2]) {
    let (slug, adapter, skill) = PACKAGES[3];
    (
        plugins::parse_package(slug, adapter, skill).unwrap(),
        [
            file("adapter.json", adapter.into()),
            file("SKILL.md", skill.into()),
        ],
    )
}
fn listing(p: &GamePackage) -> PublicationListing {
    listing_from_reply(p, &reply().to_string()).unwrap()
}

// Fixed sample of the historical Stardew Valley v1 pixel cover (indexed 192×112).
const LEGACY_PNG: &str = concat!(
    "iVBORw0KGgoAAAANSUhEUgAAAMAAAABwCAMAAAB/yI9AAAAAGFBMVEUSLCsePzhEZUwqTT8XMSsMICGMw3fw7q57sW",
    "6iAAACCElEQVR4nO3b4XKDIBAE4ESNff837hhDVTy4g4OShdt/bRX261wmzRQfDwtEpql1A2XgAVzggfAAy7cFfqTg",
    "AS7r2rqBMuCAdV0WaAI4YKu/BZYADtjrv16wBHCAq78FkgAOONeHJIAD/PpwBHAAVR+KAA4I1ecI3Aegf/uABA6I1Y",
    "cYI3AAV/9M0I5ElZFqBHhmZZr870jqH4S8XamMCvAjrV+e0AvAH4n7iJSqX5rQC0CT1PoSgvwXaICc+hKCAerWL0cY",
    "GyCt//NJDcLIAPn4hAAlCOMCUsZn/YT6uZYwKiBtfGIALWFMAFf//KJ1gBhSQxgRwI9PKkBDwAdMGZGMkHvh0m9gVH",
    "23+vyOtMuIgG35OCENcK4/X2IAuvweyVtZ+I84qv5MxgBUeSmBB7j6dHkJYhQAvbB0jOL14+U5xAiA2LKyMdLXDyN6",
    "B/CLhgjcm1l6fRrRM0C6JE2IA3Lr3xG9AtIWzPsXU279KwIfcP4ip3w6oUT9A9EH4EDkL5Z21KBE/T39APSRH7cpt6",
    "cBrpEdOSu5owH88Mcuy+7XJ2D5C3/7/dr40eP8dccALGS4Lfxrw8fvdevS1/YCoG+jl+CvpR9B0a9LXYsP4G/Lyf0x",
    "rDr7GCCc66OItXYxQCzH47j19jBAPPsj6TV3MACX2vUN0D4GaB0DtI4BWscArWOA1oEH/AIJ+dClHAG6mAAAAABJRU",
    "5ErkJggg==",
);

#[test]
fn legacy_listing_serialization_keeps_v2_approval_digest_bytes() {
    let historical = format!(
        r#"{{"title":"旧标题","summary":"旧摘要","description":"旧介绍。","tags":["模组"],"coverPng":"{LEGACY_PNG}"}}"#
    );
    let listing: PublicationListing = serde_json::from_str(&historical).unwrap();
    assert_eq!(listing.cover_png, LEGACY_PNG);
    assert_eq!(listing.cover_url, None);
    // checked_digest serializes the typed listing directly into the hash;
    // JSON object order and the absence of coverUrl are both significant.
    assert_eq!(serde_json::to_string(&listing).unwrap(), historical);
}

#[test]
fn only_exact_manifest_cover_is_allowed_for_a_new_listing() {
    let (p, originals) = fixture();
    let manifest_url = "https://Example.org/game%20cover.png?v=%2F";
    let with_cover = |url: Option<&str>| {
        let mut adapter: Value = serde_json::from_str(&originals[0].text).unwrap();
        match url {
            Some(url) => adapter["coverUrl"] = json!(url),
            None => {
                adapter.as_object_mut().unwrap().remove("coverUrl");
            }
        }
        let adapter = adapter.to_string();
        let p = plugins::parse_package(&p.adapter.slug, &adapter, &originals[1].text).unwrap();
        (p, [file("adapter.json", adapter), originals[1].clone()])
    };
    let (p, files) = with_cover(Some(manifest_url));
    let generated = listing(&p);
    assert_eq!(generated.cover_url.as_deref(), Some(manifest_url));
    assert!(generated.cover_png.is_empty());
    let serialized = serde_json::to_value(&generated).unwrap();
    assert_eq!(serialized["coverUrl"], manifest_url);
    assert!(serialized.get("coverPng").is_none());
    let rendered = render_files(&p, &files, &generated, "1.0.1").unwrap();
    assert!(rendered[0].text.contains(manifest_url));
    assert!(!rendered[1].text.contains(manifest_url));
    assert!(!rendered[1].text.contains("data:image/"));
    for replacement in [
        None,
        Some("https://example.org/game%20cover.png?v=%2F"),
        Some("https://example.org/other-game.png"),
        Some("data:image/png;base64,AAAA"),
    ] {
        let mut tampered = generated.clone();
        tampered.cover_url = replacement.map(str::to_owned);
        assert!(render_files(&p, &files, &tampered, "1.0.1").is_err());
    }
    let mut pixel = generated.clone();
    pixel.cover_png = LEGACY_PNG.into();
    assert!(render_files(&p, &files, &pixel, "1.0.1").is_err());

    let mut invalid = p.clone();
    invalid.adapter.cover_url = Some("http://example.org/cover.png".into());
    assert!(listing_from_reply(&invalid, &reply().to_string()).is_err());
    let mut tampered = generated;
    tampered.cover_url.clone_from(&invalid.adapter.cover_url);
    assert!(render_files(&invalid, &files, &tampered, "1.0.1").is_err());

    let (p, files) = with_cover(None);
    assert!(
        p.adapter.steam_app_id.is_some(),
        "Steam ID is not a cover fallback"
    );
    let missing_cover = listing(&p);
    assert_eq!(missing_cover.cover_url, None);
    let mut expected = reply();
    expected["tags"] = json!([p.adapter.name, "SMAPI", "模组管理"]);
    assert_eq!(serde_json::to_value(&missing_cover).unwrap(), expected);
    assert_eq!(
        serde_json::from_value::<PublicationListing>(expected).unwrap(),
        missing_cover
    );
    let rendered = render_files(&p, &files, &missing_cover, "1.0.1").unwrap();
    assert!(!rendered[0].text.contains("coverUrl"));
    assert_eq!(
        read_display(&rendered[1].text),
        Some(missing_cover.description.clone())
    );
    let mut injected = missing_cover;
    injected.cover_url = Some(manifest_url.into());
    assert!(render_files(&p, &files, &injected, "1.0.1").is_err());
}

#[test]
fn overlay_preserves_source_bytes_rules_and_idempotence_across_the_provider_fixtures() {
    for (slug, adapter, skill) in PACKAGES {
        // Include frontmatter-looking body text, CRLF, and significant trailing whitespace.
        let raw_skill = format!(
            "{}\n\n---\ndescription: do not replace this body line\n---\n \n",
            skill.replace("\r\n", "\n")
        )
        .replace('\n', "\r\n");
        let p = plugins::parse_package(slug, adapter, &raw_skill).unwrap();
        let orig = [
            file("adapter.json", adapter.into()),
            file("SKILL.md", raw_skill.clone()),
        ];
        let untouched = orig.clone();
        let mut l = listing(&p);
        l.summary = "安装说明: \"先核对\"。".into();
        let rendered = render_files(&p, &orig, &l, "23.0.1-beta.1+local").unwrap();
        let decoded = plugins::parse_package(slug, &rendered[0].text, &rendered[1].text).unwrap();
        assert_eq!(decoded.skill.description, l.summary);
        assert_eq!(decoded.adapter.name, l.title);
        assert_eq!(decoded.adapter.cover_url, p.adapter.cover_url);
        assert_eq!(decoded.adapter.version, "23.0.1-beta.1+local");
        assert_eq!(orig, untouched);
        let mut expected: Value = serde_json::from_str(adapter).unwrap();
        expected["name"] = json!(l.title);
        expected["version"] = json!(decoded.adapter.version);
        assert_eq!(
            expected,
            serde_json::from_str::<Value>(&rendered[0].text).unwrap()
        );
        let expected_skill = raw_skill.replacen(
            raw_skill
                .lines()
                .find(|s| s.starts_with("description:"))
                .unwrap(),
            &format!(
                "description: {}",
                serde_json::to_string(&l.summary).unwrap()
            ),
            1,
        );
        assert_eq!(display(&rendered[1].text).unwrap().original, expected_skill);
        assert_eq!(read_display(&rendered[1].text), Some(l.description.clone()));
        for f in &rendered {
            assert_eq!(f.size, f.text.len() as u64);
            assert_eq!(f.sha256, hex::encode(Sha256::digest(f.text.as_bytes())));
        }
        assert_eq!(
            render_files(&decoded, &rendered, &l, &decoded.adapter.version).unwrap(),
            rendered
        );
        l.description = "更新介绍。".into();
        let updated = render_files(&decoded, &rendered, &l, "23.0.2").unwrap();
        assert_eq!(updated[1].text.matches(BEGIN).count(), 1);
        assert!(!updated[1]
            .text
            .contains(reply()["description"].as_str().unwrap()));
        assert_eq!(display(&updated[1].text).unwrap().original, expected_skill);
    }
}

#[test]
fn listing_reserves_the_first_tag_for_the_source_game_not_the_generated_title() {
    let (mut p, _) = fixture();
    p.adapter.name = "Call of Duty: Black Ops III".into();
    let mut r = reply();
    let mut generate_tags = |tags: Value| {
        r["tags"] = tags;
        listing_from_reply(&p, &r.to_string()).unwrap().tags
    };
    assert_eq!(
        generate_tags(json!(["Windows", "BO3模组"])),
        ["Call of Duty: Black Ops III", "Windows", "BO3模组"]
    );
    assert_eq!(
        generate_tags(json!([
            "Windows",
            "call-of-duty:-black-ops-iii",
            "Call of Duty: Black Ops III"
        ])),
        ["Call of Duty: Black Ops III", "Windows"]
    );
    assert_eq!(
        generate_tags(json!(["a", "b", "c", "d", "e", "f", "g", "h"])),
        [
            "Call of Duty: Black Ops III",
            "a",
            "b",
            "c",
            "d",
            "e",
            "f",
            "g"
        ]
    );
}

#[test]
fn full_game_name_is_not_subject_to_the_supplementary_tag_length_limit() {
    let (mut p, _) = fixture();
    p.adapter.name = "The Elder Scrolls V: Skyrim Special Edition".into();
    assert_eq!(listing(&p).tags[0], p.adapter.name);
    p.adapter.name = "使命召唤：黑色行动3".into();
    assert_eq!(listing(&p).tags[0], p.adapter.name);
}

#[test]
fn rejects_unsafe_model_copy_and_ambiguous_tags() {
    let (p, _) = fixture();
    for (key, value) in [
        ("title", json!("界".repeat(86))),
        ("title", json!("abc\u{202e}def")),
        ("summary", json!("x\nname: change")),
        ("summary", json!("文".repeat(1025))),
        ("description", json!("x".repeat(4097))),
        ("description", json!("[link](https://example.org)")),
        ("description", json!("<script>alert(1)</script>")),
        ("description", json!("www.example.com")),
        ("description", json!(BEGIN)),
        ("tags", json!(["Game Mod", "game-mod"])),
        ("tags", json!(["İ", "i"])),
        ("tags", json!(["x".repeat(33)])),
    ] {
        let mut r = reply();
        r[key] = value;
        assert!(listing_from_reply(&p, &r.to_string()).is_err(), "{key}");
    }
    for raw in [
        "not JSON".into(),
        format!("```json\n{}\n```", reply()),
        "x".repeat(MAX_REPLY_BYTES + 1),
    ] {
        assert!(listing_from_reply(&p, &raw).is_err());
    }
    for (field, value) in [
        ("design", json!({"palette":"grove","icon":"sprout"})),
        ("coverPng", json!(LEGACY_PNG)),
        ("coverUrl", json!(p.adapter.cover_url)),
    ] {
        let mut extra = reply();
        extra[field] = value;
        assert!(
            listing_from_reply(&p, &extra.to_string()).is_err(),
            "{field}"
        );
    }
}

#[test]
fn imported_display_requires_one_complete_terminal_prose_footer() {
    let (p, orig) = fixture();
    let l = listing(&p);
    let rendered = render_files(&p, &orig, &l, "1.0.1").unwrap();
    let text = &rendered[1].text;
    let display = display(text).unwrap();
    assert_eq!(
        text.strip_prefix(display.original).unwrap(),
        format!("\n\n{BEGIN}\n\n{}\n{END}\n", l.description)
    );
    assert_eq!(
        read_display(&text.replace("\r\n", "\n").replace('\n', "\r\n")),
        Some(l.description.clone())
    );
    assert!(read_display(&text.replacen("name:", "unknown:", 1)).is_none());
    for tampered in [
        text.replace(END, "<!-- broken -->"),
        text.replace(END, LEGACY_END),
        format!("{text}\ninstall rule after footer"),
        text.replace(END, &format!("\n{LEGACY_COVER}{LEGACY_PNG})\n{END}")),
        text.replace(&l.description, "<img src=x>"),
        format!("{text}{text}"),
        format!(
            "{}\n{LEGACY_BEGIN}{}",
            display.original,
            text.strip_prefix(display.original).unwrap()
        ),
        format!(
            "---\nname: stardew-valley\ndescription: summary\n---{}",
            text.strip_prefix(display.original).unwrap()
        ),
    ] {
        assert!(read_display(&tampered).is_none());
        let tampered_package =
            plugins::parse_package(&p.adapter.slug, &orig[0].text, &tampered).unwrap();
        assert!(render_files(
            &tampered_package,
            &[orig[0].clone(), file("SKILL.md", tampered)],
            &l,
            "1.2.3"
        )
        .is_err());
    }
}

#[test]
fn a_legacy_footer_supplies_prose_and_is_replaced_without_touching_body() {
    let (p, orig) = fixture();
    let l = listing(&p);
    let head = format!(
        "{}\n\n保存原操作正文及尾部空格。 \n",
        p.skill_markdown.replace("\r\n", "\n")
    )
    .replace('\n', "\r\n");
    let old_description = "这是旧版介绍。\n\n第二段。";
    let legacy = format!("{head}\n\n{LEGACY_BEGIN}\n\n{old_description}\n\n{LEGACY_COVER}{LEGACY_PNG})\n{LEGACY_END}\n");
    assert_eq!(read_display(&legacy).as_deref(), Some(old_description));
    assert_eq!(display(&legacy).unwrap().original, head);
    assert_eq!(
        read_display(&legacy.replace("\r\n", "\n").replace('\n', "\r\n")).as_deref(),
        Some(old_description)
    );

    let legacy_package = plugins::parse_package(&p.adapter.slug, &orig[0].text, &legacy).unwrap();
    let rendered = render_files(
        &legacy_package,
        &[orig[0].clone(), file("SKILL.md", legacy.clone())],
        &l,
        "1.2.3",
    )
    .unwrap();
    let expected_head = head.replacen(
        head.lines()
            .find(|line| line.starts_with("description:"))
            .unwrap(),
        &format!(
            "description: {}",
            serde_json::to_string(&l.summary).unwrap()
        ),
        1,
    );
    assert_eq!(display(&rendered[1].text).unwrap().original, expected_head);
    assert_eq!(read_display(&rendered[1].text), Some(l.description.clone()));
    assert!(!rendered[1].text.contains(LEGACY_BEGIN));
    assert!(!rendered[1].text.contains(old_description));
    assert!(!rendered[1].text.contains(LEGACY_PNG));

    let png = STANDARD.decode(LEGACY_PNG).unwrap();
    let mut bad_crc = png.clone();
    bad_crc[29] ^= 1;
    let mut trailing = png.clone();
    trailing.push(42);
    let mut fake_stream = png;
    let tag = fake_stream.windows(4).position(|s| s == b"IDAT").unwrap();
    let len = u32::from_be_bytes(fake_stream[tag - 4..tag].try_into().unwrap()) as usize;
    fake_stream[tag + 4..tag + 4 + len].fill(0);
    let crc = checksum(b"IDAT", &fake_stream[tag + 4..tag + 4 + len]);
    fake_stream[tag + 4 + len..tag + 8 + len].copy_from_slice(&crc.to_be_bytes());
    for bad in [
        STANDARD.encode(bad_crc),
        STANDARD.encode(trailing),
        STANDARD.encode(fake_stream),
        STANDARD.encode(b"\x89PNG\r\n\x1a\n"),
        "A".repeat(MAX_PNG_BYTES * 2),
        "data:image/svg+xml,x".into(),
    ] {
        let damaged = legacy.replace(LEGACY_PNG, &bad);
        assert!(read_display(&damaged).is_none());
        let damaged_package =
            plugins::parse_package(&p.adapter.slug, &orig[0].text, &damaged).unwrap();
        assert!(render_files(
            &damaged_package,
            &[orig[0].clone(), file("SKILL.md", damaged)],
            &l,
            "1.2.3"
        )
        .is_err());
    }
}

#[test]
fn validates_provenance_and_handles_provider_schema_without_losing_operations() {
    let (p, orig) = fixture();
    let l = listing(&p);
    let mut stale = orig.clone();
    stale[1].sha256 = "0".repeat(64);
    assert!(render_files(&p, &stale, &l, "1.0.0").is_err());
    let mut swapped = p.clone();
    swapped.adapter.mod_types[0].notes.push('x');
    assert!(render_files(&swapped, &orig, &l, "1.0.0").is_err());
    assert!(render_files(&p, &orig, &l, "2").is_err());
    let mut json: Value = serde_json::from_str(&orig[0].text).unwrap();
    json["mcpServers"] = json!([]);
    let adapter = json.to_string();
    let empty = plugins::parse_package(&p.adapter.slug, &adapter, &p.skill_markdown).unwrap();
    let copy = render_files(
        &empty,
        &[file("adapter.json", adapter), orig[1].clone()],
        &l,
        "1.0.2",
    )
    .unwrap();
    assert!(serde_json::from_str::<Value>(&copy[0].text)
        .unwrap()
        .get("mcpServers")
        .is_none());
    json["mcpServers"] = json!([{"id":"docs","name":"Docs","transport":"streamable_http","url":"https://example.org/mcp"}]);
    let declared: plugins::GameAdapter = serde_json::from_value(json).unwrap();
    let declared = GamePackage {
        adapter: declared,
        ..p
    };
    assert!(
        matches!(render_files(&declared,&orig,&l,"1.0.2"),Err(AppError::Invalid(msg)) if msg.contains("mcpServers"))
    );
}

struct FixtureSecret;
impl SecretStore for FixtureSecret {
    fn read(&self, key: &str) -> AppResult<Option<String>> {
        assert_eq!(key, "model.publication-test");
        Ok(Some("fake-model-key".into()))
    }
    fn write(&self, key: &str, _: &str) -> AppResult<()> {
        assert_eq!(key, "model.publication-test");
        Ok(())
    }
}
fn settings(root: &std::path::Path, url: String) -> Settings {
    let s = Settings::with_secret_store(
        Db::open(&root.join("settings.sqlite")).unwrap(),
        Arc::new(FixtureSecret),
    );
    s.write_secret("model.publication-test", "fake-model-key")
        .unwrap();
    s.save(&AppSettings {
        base_url: url,
        model: "saved-default".into(),
        api_key_ref: Some("model.publication-test".into()),
        models: vec![AgentModelSettings {
            display_name: "Fixture".into(),
            id: "selected-model".into(),
            reasoning_levels: vec![ReasoningEffort::Low],
        }],
        ..Default::default()
    })
    .unwrap();
    s.save_composer_preferences(&ComposerPreferences {
        model: Some("selected-model".into()),
        reasoning_effort: Some(ReasoningEffort::Low),
        permission_mode: None,
    })
    .unwrap();
    s
}

fn sse(delta: Value, reason: &str) -> ([(&'static str, &'static str); 1], String) {
    let event = json!({"choices":[{"delta":delta,"finish_reason":reason}]});
    (
        [("content-type", "text/event-stream")],
        format!("data: {event}\n\ndata: [DONE]\n\n"),
    )
}

#[tokio::test]
async fn configured_model_receives_only_bounded_structured_facts_and_no_tools() {
    let received = Arc::new(Mutex::new(None));
    let capture = received.clone();
    let handler = move |h: HeaderMap, body: Bytes| async move {
        let body: Value = serde_json::from_slice(&body).unwrap();
        *capture.lock().unwrap() = Some((h, body));
        sse(json!({"content":reply().to_string()}), "stop")
    };
    let app = Router::new().route("/v1/chat/completions", post(handler));
    let listen = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listen.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listen, app).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let settings = settings(root.path(), format!("http://{addr}/v1"));
    let (mut p, _) = fixture();
    p.adapter.name = r"C:\Users\private\identity".into();
    let err = generate(&settings, &p, &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("游戏名称标签"));
    assert!(received.lock().unwrap().is_none());
    p.adapter.name = "Stardew Valley".into();
    p.skill.description = "PRIVATE_FRONTMATTER".into();
    p.skill_markdown = "PRIVATE_INSTRUCTIONS".into();
    p.adapter.cover_url = Some("https://private.example.test".into());
    p.adapter.install_roots[0].notes = "PRIVATE_ROOT".into();
    p.adapter.sources[0].title = "PRIVATE_REFERENCE".into();
    p.adapter.mod_types = vec![p.adapter.mod_types[0].clone(); 1000];
    let listing = generate(&settings, &p, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(listing.title, reply()["title"]);
    assert_eq!(listing.cover_url, p.adapter.cover_url);
    assert!(listing.cover_png.is_empty());
    server.abort();
    let (headers, body) = received.lock().unwrap().take().unwrap();
    assert_eq!(headers["authorization"], "Bearer fake-model-key");
    assert_eq!(body["model"], "selected-model");
    assert_eq!(body["reasoning_effort"], "low");
    assert!(body.get("tools").is_none());
    assert_eq!(body["messages"].as_array().unwrap().len(), 2);
    let facts = body["messages"][1]["content"].as_str().unwrap();
    let facts: Value = serde_json::from_str(facts).unwrap();
    assert_eq!(facts["name"], "Stardew Valley");
    assert_eq!(facts["modTypes"].as_array().unwrap().len(), 16);
    assert_eq!(facts["slug"], p.adapter.slug);
    for needle in [
        "private",
        "PRIVATE_",
        "fake-model-key",
        "discoveryHints",
        "relativePath",
        "https://",
        "design",
        "palette",
        "coverUrl",
        "像素",
    ] {
        assert!(!body.to_string().contains(needle));
    }
    settings
        .save_composer_preferences(&ComposerPreferences {
            model: Some("removed-model".into()),
            reasoning_effort: Some(ReasoningEffort::High),
            permission_mode: None,
        })
        .unwrap();
    let fallback = model_config(&settings).unwrap();
    assert_eq!(fallback.model, "saved-default");
    assert_eq!(fallback.reasoning_effort, None);
}

#[tokio::test]
async fn generation_reports_model_errors_cancellation_and_a_stalled_provider_deadline() {
    let listen = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listen.local_addr().unwrap();
    let tool_reply = || async {
        let calls = json!([{"index":0,"id":"x","function":{"name":"test","arguments":"{}"}}]);
        sse(json!({"tool_calls":calls}), "tool_calls")
    };
    let echo = || async {
        let mut value = reply();
        value["title"] = json!("fake-model-key");
        sse(json!({"content":value.to_string()}), "stop")
    };
    let app = Router::new()
        .route(
            "/failure/chat/completions",
            post(|| async { (StatusCode::UNAUTHORIZED, "invalid model key") }),
        )
        .route(
            "/stall/chat/completions",
            post(std::future::pending::<String>),
        )
        .route("/echo/chat/completions", post(echo))
        .route("/tools/chat/completions", post(tool_reply));
    let server = tokio::spawn(async move { axum::serve(listen, app).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let settings = settings(root.path(), format!("http://{addr}/failure"));
    let (p, _) = fixture();
    let cancel = CancellationToken::new();
    let error = generate(&settings, &p, &cancel)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("401"));
    assert!(!error.contains("fake-model-key"));
    let mut config = settings.load().unwrap();
    config.base_url = format!("http://{addr}/tools");
    settings.save(&config).unwrap();
    assert!(generate(&settings, &p, &cancel)
        .await
        .unwrap_err()
        .to_string()
        .contains("工具"));
    config.base_url = format!("http://{addr}/echo");
    settings.save(&config).unwrap();
    assert!(
        matches!(generate(&settings,&p,&cancel).await,Err(AppError::Denied(msg)) if !msg.contains("fake-model-key"))
    );
    config.base_url = format!("http://{addr}/stall");
    settings.save(&config).unwrap();
    assert!(
        generate_with_deadline(&settings, &p, &cancel, Duration::from_millis(150))
            .await
            .unwrap_err()
            .to_string()
            .contains("超时")
    );
    let (result, ()) = tokio::join!(generate(&settings, &p, &cancel), async {
        tokio::time::sleep(Duration::from_millis(40)).await;
        cancel.cancel()
    });
    assert!(matches!(result, Err(AppError::Cancelled)));
    server.abort();
}

#[test]
fn auto_publication_supplies_game_tag_and_requires_a_catalog_readable_release() {
    let (package, mut originals) = fixture();
    let mut data = reply();
    data["tags"] = json!([]);
    assert_eq!(
        listing_from_reply(&package, &data.to_string())
            .unwrap()
            .tags,
        [package.adapter.name.as_str()]
    );
    let mut skill = originals[1].text.clone();
    skill.extend(std::iter::repeat_n(
        ' ',
        crate::plugin_workflow::SKILL_LIMIT - skill.len(),
    ));
    let package =
        plugins::parse_package(&package.adapter.slug, &originals[0].text, &skill).unwrap();
    originals[1] = file("SKILL.md", skill);
    assert!(
        render_files(&package, &originals, &listing(&package), "1.2.0")
            .unwrap_err()
            .to_string()
            .contains("云目录")
    );
}
