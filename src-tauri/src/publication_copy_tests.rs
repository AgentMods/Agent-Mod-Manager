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
use std::{
    io::Read,
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;

fn reply() -> Value {
    json!({"title":"Stardew Valley 模组适配", "summary":"SMAPI：安全管理模组。", "description":"识别游戏并核对模组类型。\n\n安装前可检查依赖和存档。", "tags":["SMAPI","模组管理"], "design":{"palette":"grove","icon":"sprout"}})
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

// Independent scanline decode for comparison, including the network-order chunk
// lengths and PNG palette. Never use the generator's pixels as the test result.
fn decode_rgb(encoded: &str) -> Vec<u8> {
    let bytes = STANDARD.decode(encoded).unwrap();
    assert!(bytes.len() < 2048);
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    let mut pos = 8;
    let mut palette = vec![];
    let mut raw = vec![];
    let mut dimensions = (0, 0);
    while pos < bytes.len() {
        let length = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        let data = &bytes[pos + 8..pos + 8 + length];
        match &bytes[pos + 4..pos + 8] {
            b"IHDR" => {
                dimensions = (
                    u32::from_be_bytes(data[..4].try_into().unwrap()) as usize,
                    u32::from_be_bytes(data[4..8].try_into().unwrap()) as usize,
                );
                assert_eq!(&data[8..], &[8, 3, 0, 0, 0]);
            }
            b"PLTE" => palette = data.to_vec(),
            b"IDAT" => {
                flate2::read::ZlibDecoder::new(data)
                    .read_to_end(&mut raw)
                    .unwrap();
            }
            b"IEND" => assert_eq!(pos + 12, bytes.len()),
            _ => panic!("unexpected PNG chunk"),
        }
        pos += length + 12;
    }
    let (w, h) = dimensions;
    assert_eq!((w, h), (192, 112));
    assert_eq!(raw.len(), (w + 1) * h);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for row in raw.chunks_exact(w + 1) {
        assert_eq!(row[0], 0);
        for index in &row[1..] {
            let i = usize::from(*index) * 3;
            rgb.extend_from_slice(&palette[i..i + 3]);
        }
    }
    rgb
}

#[test]
fn model_draws_different_pixels_for_the_same_package_and_palette() {
    let (package, _) = fixture();
    let mut output = reply();
    output["design"]["rows"] = json!([
        "00030000", "00233000", "02332100", "23222110", "02322100", "00221000", "00010000",
        "00000000"
    ]);
    let first = listing_from_reply(&package, &output.to_string()).unwrap();
    let rgb1 = decode_rgb(&first.cover_png);
    output["design"]["rows"][0] = json!("00020000");
    let second = listing_from_reply(&package, &output.to_string()).unwrap();
    let rgb2 = decode_rgb(&second.cover_png);
    assert_eq!(
        rgb1.as_chunks::<3>()
            .0
            .iter()
            .zip(rgb2.as_chunks::<3>().0.iter())
            .filter(|(a, b)| a != b)
            .count(),
        16
    );
    assert_ne!(decode_rgb(&listing(&package).cover_png), rgb1);
    output["design"].as_object_mut().unwrap().remove("icon");
    assert_eq!(
        listing_from_reply(&package, &output.to_string()).unwrap(),
        second
    );
    assert_eq!(
        serde_json::to_value(&first).unwrap()["coverPng"],
        first.cover_png
    );
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
        assert_eq!(
            read_display(&rendered[1].text),
            Some((l.description.clone(), l.cover_png.clone()))
        );
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
fn rejects_unsafe_model_copy_designs_and_ambiguous_tags() {
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
        (
            "design",
            json!({"palette":"grove","icon":"https://evil.test/cover"}),
        ),
        ("design", json!({"palette":"grove","rows":[]})),
    ] {
        let mut r = reply();
        r[key] = value;
        assert!(listing_from_reply(&p, &r.to_string()).is_err(), "{key}");
    }
    for rows in [
        vec!["00000000"; 8],
        vec!["00001111"; 7],
        vec!["00000004"; 8],
        vec!["111"; 8],
    ] {
        let mut r = reply();
        r["design"]["rows"] = json!(rows);
        assert!(listing_from_reply(&p, &r.to_string()).is_err());
    }
    for raw in [
        "not JSON".into(),
        format!("```json\n{}\n```", reply()),
        "x".repeat(MAX_REPLY_BYTES + 1),
    ] {
        assert!(listing_from_reply(&p, &raw).is_err());
    }
    let mut extra = reply();
    extra["coverUrl"] = json!("https://example.org/");
    assert!(listing_from_reply(&p, &extra.to_string()).is_err());
}

#[test]
fn imported_display_requires_a_complete_safe_footer_and_bounded_png() {
    let (p, orig) = fixture();
    let l = listing(&p);
    let rendered = render_files(&p, &orig, &l, "1.0.1").unwrap();
    let text = &rendered[1].text;
    assert_eq!(
        read_display(&text.replace("\r\n", "\n").replace('\n', "\r\n")),
        Some((l.description.clone(), l.cover_png.clone()))
    );
    let png = STANDARD.decode(&l.cover_png).unwrap();
    let mut bad_crc = png.clone();
    bad_crc[29] ^= 1;
    let mut trailing = png.clone();
    trailing.push(42);
    let mut fake_stream = png.clone();
    let tag = png.windows(4).position(|s| s == b"IDAT").unwrap();
    let len = u32::from_be_bytes(png[tag - 4..tag].try_into().unwrap()) as usize;
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
        assert!(read_display(&text.replace(&l.cover_png, &bad)).is_none());
        let mut hostile = l.clone();
        hostile.cover_png = bad;
        assert!(render_files(&p, &orig, &hostile, "1.0.1").is_err());
    }
    assert!(read_display(&text.replacen("name:", "unknown:", 1)).is_none());
    for tampered in [
        text.replace(END, "<!-- broken -->"),
        format!("{text}\ninstall rule after footer"),
        text.replace(COVER, "![x](https://"),
        text.replace(&l.description, "<img src=x>"),
        format!("{text}{text}"),
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
    server.abort();
    let (headers, body) = received.lock().unwrap().take().unwrap();
    assert_eq!(headers["authorization"], "Bearer fake-model-key");
    assert_eq!(body["model"], "selected-model");
    assert_eq!(body["reasoning_effort"], "low");
    assert!(body.get("tools").is_none());
    assert_eq!(body["messages"].as_array().unwrap().len(), 2);
    let facts = body["messages"][1]["content"].as_str().unwrap();
    let facts: Value = serde_json::from_str(facts).unwrap();
    assert!(facts["name"].is_null());
    assert_eq!(facts["modTypes"].as_array().unwrap().len(), 16);
    assert_eq!(facts["slug"], p.adapter.slug);
    for needle in [
        "private",
        "PRIVATE_",
        "fake-model-key",
        "discoveryHints",
        "relativePath",
        "https://",
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
fn auto_publication_requires_tags_and_a_catalog_readable_release() {
    let (package, mut originals) = fixture();
    let mut data = reply();
    data["tags"] = json!([]);
    assert!(listing_from_reply(&package, &data.to_string()).is_err());
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
