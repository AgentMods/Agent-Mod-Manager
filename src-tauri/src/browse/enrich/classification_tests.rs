use super::*;

fn input<'a>(id: &'a str, title: &'a str) -> EnrichmentInput<'a> {
    EnrichmentInput {
        source: "source-private-id",
        item_id: id,
        title,
        summary: None,
        kind: Some("Miscellaneous"),
        tags: &[],
    }
}

#[tokio::test]
async fn classifies_even_source_typed_mods_and_reuses_types_independently_of_translation() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("categories.sqlite");
    let db = Db::open(&file).unwrap();
    let cancel = CancellationToken::new();
    let inputs = [
        input("private-id-a", "Garage Inspector"),
        EnrichmentInput {
            kind: Some("Audio"),
            ..input("private-id-b", "City dashboard")
        },
    ];
    let types = classify_with(&db, &inputs, &cancel, |payload| async move {
        let request: Value = serde_json::from_str(&payload).unwrap();
        assert!(!payload.contains("private-id"));
        assert!(request.get("targetLocale").is_none());
        assert_eq!(request["items"].as_array().unwrap().len(), 2);
        assert_eq!(request["items"][1]["kind"], "Audio");
        Some(
            json!({"items": [{"index":1,"typeKey":"interface"},{"index":0,"typeKey":"tools"}]})
                .to_string(),
        )
    })
    .await;
    assert_eq!(types, [Some(ModType::Tools), Some(ModType::Interface)]);
    drop(db);
    let db = Db::open(&file).unwrap();
    let inputs = [
        EnrichmentInput {
            summary: Some("Updated mod"),
            ..inputs[0]
        },
        inputs[1],
    ];
    let types = classify_with(&db, &inputs, &cancel, |payload| async move {
        let request: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(request["items"].as_array().unwrap().len(), 1);
        assert_eq!(request["items"][0]["index"], 0);
        Some(json!({"items":[{"index":0,"typeKey":"fixes"}]}).to_string())
    })
    .await;
    assert_eq!(types, [Some(ModType::Fixes), Some(ModType::Interface)]);
}

#[tokio::test]
async fn invalid_or_missing_categories_do_not_discard_siblings_or_poison_later_searches() {
    let temp = tempfile::tempdir().unwrap();
    let db = Db::open(&temp.path().join("categories.sqlite")).unwrap();
    let cancel = CancellationToken::new();
    let inputs = [
        input("a", "A"),
        input("b", "B"),
        input("c", "C"),
        input("d", "D"),
        input("e", "E"),
    ];
    let types = classify_with(&db, &inputs, &cancel, |_| async {
        Some(
            json!({"items":[
                {"index":3,"typeKey":"fixes"},{"index":0,"typeKey":"tools"},
                {"index":3,"typeKey":"visual"},{"index":1,"typeKey":"unknown"},
                {"index":2,"typeKey":"user-invented"},{"index":77,"typeKey":"visual"}
            ]})
            .to_string(),
        )
    })
    .await;
    assert_eq!(types, [Some(ModType::Tools), None, None, None, None]);
    let retry = classify_with(&db, &inputs, &cancel, |payload| async move {
        let request: Value = serde_json::from_str(&payload).unwrap();
        let pending: Vec<_> = request["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["index"].as_u64().unwrap())
            .collect();
        assert_eq!(pending, [1, 2, 3, 4]);
        None
    })
    .await;
    assert_eq!(retry, types);
}

#[tokio::test]
async fn full_page_fits_including_escaped_metadata_and_cancellation_keeps_cache_clean() {
    let temp = tempfile::tempdir().unwrap();
    let db = Db::open(&temp.path().join("categories.sqlite")).unwrap();
    let cancel = CancellationToken::new();
    let long = "\"\\\u{1f}汉".repeat(400);
    let tags = vec!["\\".repeat(300); 8];
    let inputs = [EnrichmentInput {
        summary: Some(&long),
        kind: Some(&long),
        tags: &tags,
        ..input("id", &long)
    }; MAX_ITEMS];
    let signal = &cancel;
    let types = classify_with(&db, &inputs, &cancel, |payload| async move {
        assert!(payload.len() <= MAX_INPUT_BYTES);
        let request: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(request["items"].as_array().unwrap().len(), MAX_ITEMS);
        signal.cancel();
        Some(json!({"items":[{"index":0,"typeKey":"other"}]}).to_string())
    })
    .await;
    assert_eq!(types, [None; MAX_ITEMS]);
    let retried = classify_with(&db, &inputs[..1], &CancellationToken::new(), |_| async {
        Some(json!({"items":[{"index":0,"typeKey":"other"}]}).to_string())
    })
    .await;
    assert_eq!(retried, [Some(ModType::Other)]);
}
