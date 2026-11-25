use super::*;
use axum::{body::Body, extract::Request, response::Response, routing::post, Router};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

fn memory() -> Db {
    Db(Arc::new(Mutex::new(Connection::open_in_memory().unwrap())))
}

fn test_item<'a>(tags: &'a [String]) -> EnrichmentInput<'a> {
    EnrichmentInput {
        source: "offline-source",
        item_id: "private-opaque-item-key",
        title: "Enhanced Armor",
        summary: Some("Improves helmet textures."),
        kind: None,
        tags,
    }
}

fn reply(index: usize, title: &str, label: &str) -> Value {
    json!({
        "index":index,
        "title":title,
        "summary":"Localized summary.",
        "typeKey":"visual",
        "tags":[{"key":"textures","label":label}]
    })
}

#[test]
fn stable_taxonomy_converges_on_known_tags_without_losing_provider_labels() {
    for (label, target) in [
        ("Texture Packs and Textures", "textures"),
        ("超高清材质", "raw"),
        ("材质包", "textures"),
        ("テクスチャ", "textures"),
        ("Texturen", "textures"),
        ("FR: Corbeau", "raw"),
    ] {
        let tags = [label.to_owned()];
        let item = fallback_item(test_item(&tags));
        assert_eq!(item.tags[0].label, label);
        if target == "raw" {
            assert!(item.tags[0].key.starts_with("raw-"));
        } else {
            assert_eq!(item.tags[0].key, target);
            assert_eq!(item.type_key, ModType::Visual);
        }
    }
    let tags = ["Co-op".to_owned(), "联机".to_owned(), "CO OP".to_owned()];
    assert_eq!(fallback_item(test_item(&tags)).tags.len(), 1);
    assert_eq!(fallback_item(test_item(&[])).type_key.as_str(), "unknown");
    assert_eq!(
        fallback_item(EnrichmentInput {
            kind: Some("Miscellaneous"),
            ..test_item(&[])
        })
        .type_key,
        ModType::Other
    );
    for kind in ["Gameplay Effects and Changes", "玩法", "ゲームプレイ"] {
        let item = fallback_item(EnrichmentInput {
            kind: Some(kind),
            ..test_item(&[])
        });
        assert_eq!(serde_json::to_value(item).unwrap()["typeKey"], "gameplay");
    }
    let conflicting = ["Models".to_owned(), "Music".to_owned()];
    assert_eq!(
        fallback_item(test_item(&conflicting)).type_key,
        ModType::Unknown
    );
}

#[tokio::test]
async fn metadata_cache_is_durable_and_tracks_language_rules_and_full_original() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("browse.sqlite");
    let database = Db::open(&file).unwrap();
    let cancel = CancellationToken::new();
    let input = test_item(&[]);
    app_locale::set(&database, Some("zh-CN".into())).unwrap();
    let locale = app_locale::effective(&database);
    let first = enrich_with(
        &database,
        &[input],
        &locale,
        &cancel,
        |payload| async move {
            let packet: Value = serde_json::from_str(&payload).unwrap();
            assert_eq!(packet["targetLocale"], "zh-CN");
            assert_eq!(packet["items"][0]["title"], input.title);
            assert!(!payload.contains(input.source));
            assert!(!payload.contains(input.item_id));
            assert_eq!(packet["items"][0].as_object().unwrap().len(), 5);
            Some(json!({"items":[reply(0, "增强护甲", "材质")]}).to_string())
        },
    )
    .await;
    assert_eq!(first[0].title, "增强护甲");
    assert_eq!(first[0].tags[0].key, "textures");
    drop(database);

    let database = Db::open(&file).unwrap();
    let hit = enrich_with(&database, &[input], "zh-CN", &cancel, |_| async {
        panic!("cache miss")
    })
    .await;
    assert_eq!(hit, first);
    app_locale::set(&database, Some("ja".into())).unwrap();
    let japanese = enrich_with(
        &database,
        &[input],
        &app_locale::effective(&database),
        &cancel,
        |_| async {
            Some(json!({"items":[reply(0, "改良型防具", "テクスチャ")]}).to_string())
        },
    )
    .await;
    assert_eq!(japanese[0].tags[0].key, first[0].tags[0].key);
    assert_ne!(japanese[0].tags[0].label, first[0].tags[0].label);
    let update = EnrichmentInput {
        summary: Some("A new release."),
        ..input
    };
    let result = enrich_with(&database, &[update], "zh-CN", &cancel, |_| async { None }).await;
    assert_eq!(result[0], fallback_item(update));
    assert_eq!(
        enrich_with(&database, &[input], "zh-CN", &cancel, |_| async {
            panic!("Chinese cache overwritten")
        })
        .await,
        first
    );
}

#[tokio::test]
async fn malformed_or_unavailable_enrichment_never_poison_cache_or_replace_original() {
    let db = memory();
    let cancel = CancellationToken::new();
    let input = test_item(&[]);
    let row = reply(0, "Local", "Textures");
    for invalid in [
        None,
        Some("{broken}".into()),
        Some(json!({"items":[row.clone(), row.clone()]}).to_string()),
        Some(json!({"items":[reply(7, "Unknown item", "Textures")]}).to_string()),
        Some(json!({"items":[{"index":0,"title":"Wrong","typeKey":"visual","tags":[]}]}).to_string()),
        Some(json!({"items":[{"index":0,"title":"Wrong","summary":null,"typeKey":"visual","tags":[]}]}).to_string()),
        Some(json!({"items":[{"index":0,"title":"Wrong","summary":"Wrong","typeKey":"visual","tags":[{"key":"invented-key","label":"Fake"}]}]}).to_string()),
    ] {
        assert_eq!(
            enrich_with(&db, &[input], "zh-CN", &cancel, |_| async { invalid }).await[0],
            fallback_item(input)
        );
    }
    let fixed = enrich_with(&db, &[input], "zh-CN", &cancel, |_| async {
        Some(format!("```json\r\n{}\r\n```", json!({"items":[row]})))
    })
    .await;
    assert_eq!(fixed[0].title, "Local");
    // A cancelled model result is not published or cached.
    let result = enrich_with(&db, &[input], "en", &cancel, |_| async {
        cancel.cancel();
        Some(json!({"items":[reply(0, "Cancelled", "Textures")]}).to_string())
    })
    .await;
    assert_eq!(result[0], fallback_item(input));
    assert_eq!(
        enrich_with(&db, &[input], "en", &CancellationToken::new(), |_| async {
            None
        })
        .await[0],
        fallback_item(input)
    );
}

#[tokio::test]
async fn batched_prompt_is_size_bounded_and_reply_rows_keep_input_order() {
    let notes = "汉".repeat(MAX_SUMMARY_BYTES);
    let inputs: Vec<_> = (0..MAX_ITEMS + 8)
        .map(|_| EnrichmentInput {
            summary: Some(&notes),
            ..test_item(&[])
        })
        .collect();
    let out = enrich_with(
        &memory(),
        &inputs,
        "en",
        &CancellationToken::new(),
        |payload| async move {
            assert!(payload.len() <= MAX_INPUT_BYTES);
            let parsed: Value = serde_json::from_str(&payload).unwrap();
            assert_eq!(parsed["items"].as_array().unwrap().len(), MAX_ITEMS);
            assert!(parsed["items"][0]["summary"].as_str().unwrap().len() <= MAX_SUMMARY_BYTES);
            Some(
                json!({"items":[reply(1,"second","Textures"),reply(0,"first","Textures")]})
                    .to_string(),
            )
        },
    )
    .await;
    assert_eq!(out.len(), inputs.len());
    assert_eq!(out[0].title, "first");
    assert_eq!(out[1].title, "second");
    assert_eq!(out[MAX_ITEMS].summary.as_ref(), Some(&notes));
    // Escaping is included in the cap, not just UTF-8 field lengths.
    let escaped = "\0".repeat(MAX_SUMMARY_BYTES);
    let inputs = vec![
        EnrichmentInput {
            summary: Some(&escaped),
            ..test_item(&[])
        };
        MAX_ITEMS
    ];
    enrich_with(
        &memory(),
        &inputs,
        "en",
        &CancellationToken::new(),
        |payload| async move {
            assert!(payload.len() <= MAX_INPUT_BYTES);
            let parsed: Value = serde_json::from_str(&payload).unwrap();
            assert!(parsed["items"].as_array().unwrap().len() < MAX_ITEMS);
            None
        },
    )
    .await;
}

#[tokio::test]
async fn query_translation_is_optional_and_cached_by_effective_and_source_languages() {
    let db = memory();
    let cancel = CancellationToken::new();
    let query = "光影 Iris";
    let first = translate_with(&db, query, "zh-CN", "en", &cancel, |payload| async move {
        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            parsed,
            json!({"appLocale":"zh-CN","targetLocale":"en","query":query})
        );
        Some(json!({"query":"Iris shaders"}).to_string())
    })
    .await;
    assert_eq!(first.as_deref(), Some("Iris shaders"));
    assert_eq!(
        translate_with(&db, query, "zh-CN", "en", &cancel, |_| async {
            panic!("cache miss");
        })
        .await,
        first
    );
    assert_eq!(
        translate_with(&db, query, "zh-CN", "zh-CN", &cancel, |_| async {
            Some(json!({"query":query}).to_string())
        })
        .await,
        None
    );
    assert_eq!(
        translate_with(&db, query, "zh-CN", "zh-CN", &cancel, |_| async {
            panic!("unchanged result should cache");
        })
        .await,
        None
    );
    assert_eq!(
        translate_with(&db, query, "ja", "en", &cancel, |_| async { None }).await,
        None
    );
    for invalid in [" ", "http://example.test/a", "2026", "malicious\nURL"] {
        assert_eq!(
            translate_with(&db, invalid, "zh-CN", "en", &cancel, |_| async {
                panic!("invalid input cannot send a model request");
            })
            .await,
            None
        );
    }
    assert_eq!(
        translate_with(&db, "预设", "zh-CN", "en", &cancel, |_| async {
            Some(json!({"query":"https://example.test/result"}).to_string())
        })
        .await,
        None
    );
    assert_eq!(
        translate_with(&db, "预设", "zh-CN", "en", &cancel, |_| async {
            Some(json!({"query":"presets"}).to_string())
        })
        .await
        .as_deref(),
        Some("presets")
    );
}

#[tokio::test]
async fn model_work_can_wait_behind_queries_but_the_queue_is_bounded_and_cancelable() {
    let active = Semaphore::new(2);
    let queue = Semaphore::new(1);
    let reserved = active.acquire_many(2).await.unwrap();
    let cancel = CancellationToken::new();
    let mut pending = std::pin::pin!(acquire_model_slot(&queue, &active, &cancel));
    assert!(futures_util::poll!(pending.as_mut()).is_pending());
    assert!(acquire_model_slot(&queue, &active, &cancel).await.is_none());
    drop(reserved);
    assert!(pending.await.is_some());
    assert_eq!(queue.available_permits(), 1);

    let reserved = active.acquire_many(2).await.unwrap();
    let mut pending = std::pin::pin!(acquire_model_slot(&queue, &active, &cancel));
    assert!(futures_util::poll!(pending.as_mut()).is_pending());
    cancel.cancel();
    assert!(pending.await.is_none());
    drop(reserved);
    assert_eq!(queue.available_permits(), 1);
}

#[tokio::test]
async fn configured_client_attempts_only_one_tool_free_request_and_rejects_credential_echo() {
    const KEY: &str = "offline-key-marker";
    struct FixtureSecretStore(Mutex<Option<String>>);
    impl crate::settings::SecretStore for FixtureSecretStore {
        fn read(&self, _: &str) -> crate::error::AppResult<Option<String>> {
            Ok(self.0.lock().unwrap().clone())
        }

        fn write(&self, _: &str, value: &str) -> crate::error::AppResult<()> {
            *self.0.lock().unwrap() = Some(value.to_owned());
            Ok(())
        }
    }
    let valid_reply = json!({"query":"shaders"}).to_string();
    // JSON's unicode escape must not bypass credential filtering.
    let credential_reply = r#"{"query":"offl\u0069ne-key-marker"}"#.to_owned();
    let to_sse = |text: String| {
        format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":"stop"}]})
        )
    };
    let unsolicited_tool = format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{
            "index":0,
            "delta":{"content":valid_reply,"tool_calls":[{
                "index":0,"id":"unexpected-call",
                "function":{"name":"unused_tool","arguments":"{}"}
            }]},
            "finish_reason":"tool_calls"
        }]})
    );
    let replies = Arc::new(Mutex::new(std::collections::VecDeque::from([
        to_sse(valid_reply.clone()),
        String::new(), // premature EOF must not consume a future reply as a retry
        to_sse(credential_reply),
        unsolicited_tool,
    ])));
    let server_replies = replies.clone();
    let route = post(move |request: Request| {
        let replies = server_replies.clone();
        async move {
            assert_eq!(
                request.headers().get("Authorization").unwrap(),
                format!("Bearer {KEY}").as_str()
            );
            let bytes = axum::body::to_bytes(request.into_body(), MAX_INPUT_BYTES + 2048)
                .await
                .unwrap();
            let data: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(data.get("tools").is_none());
            assert_eq!(data["messages"].as_array().unwrap().len(), 2);
            assert_eq!(data["messages"][0]["role"], "system");
            assert_eq!(data["messages"][1]["role"], "user");
            let packet: Value =
                serde_json::from_str(data["messages"][1]["content"].as_str().unwrap()).unwrap();
            assert_eq!(packet["appLocale"], "zh-CN");
            assert_eq!(packet["targetLocale"], "en");
            let text = replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected retry");
            Response::builder()
                .header("Content-Type", "text/event-stream")
                .body(Body::from(text))
                .unwrap()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let db = Db::open(&directory.path().join("browse.sqlite")).unwrap();
    app_locale::set(&db, Some("zh-CN".into())).unwrap();
    let settings = Settings::with_secret_store(db, Arc::new(FixtureSecretStore(Mutex::new(None))));
    settings
        .save(&crate::settings::AppSettings {
            base_url: format!("http://{}/v1", listener.local_addr().unwrap()),
            model: "offline-model".into(),
            ..Default::default()
        })
        .unwrap();
    settings
        .write_secret(crate::settings::MODEL_API_KEY, KEY)
        .unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/v1/chat/completions", route))
            .await
            .unwrap();
    });
    let cancel = CancellationToken::new();
    let ask = |query: &'static str| translate_query(&settings, query, "en", &cancel);
    assert_eq!(ask(KEY).await, None);
    assert_eq!(replies.lock().unwrap().len(), 4);
    assert_eq!(ask("着色器").await.as_deref(), Some("shaders"));
    assert_eq!(ask("着色器").await.as_deref(), Some("shaders")); // SQLite hit
    assert_eq!(replies.lock().unwrap().len(), 3);
    assert_eq!(ask("护甲").await, None); // premature EOF: no retry
    assert_eq!(replies.lock().unwrap().len(), 2);
    assert_eq!(ask("材质").await, None); // escaped credential echo
    assert_eq!(ask("音乐").await, None); // unsolicited tool response
    assert!(replies.lock().unwrap().is_empty());
    server.abort();
}
