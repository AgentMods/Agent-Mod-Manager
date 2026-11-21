use super::*;
use serde_json::json;

fn query(value: impl Into<String>, count: u8) -> WebSearchQuery {
    WebSearchQuery {
        query: value.into(),
        count,
    }
}

#[test]
fn validates_query_limits_and_public_url_terms_without_false_path_matches() {
    query("x", 1).validate().unwrap();
    query("x".repeat(MAX_QUERY_CHARS), 10).validate().unwrap();
    query("界".repeat(MAX_QUERY_CHARS), 5).validate().unwrap();
    query("word ".repeat(74) + "word", 5).validate().unwrap();
    query("https://docs.rs/serde/latest/serde/", 5)
        .validate()
        .unwrap();
    query("find https://example.com/search?q=/guide/", 5)
        .validate()
        .unwrap();

    for invalid in [
        query("", 1),
        query("x".repeat(MAX_QUERY_CHARS + 1), 5),
        query("word ".repeat(75) + "word", 5),
        query("x", 0),
        query("x", 11),
        query("/Users/alice/private.txt", 5),
        query(r#"C:\Users\alice\private.txt"#, 5),
        query("file:///etc/passwd", 5),
        query("https://localhost/private", 5),
        query("https://127.0.0.1/private", 5),
        query("https://user:password@example.com/private", 5),
    ] {
        assert!(
            invalid.validate().is_err(),
            "unexpectedly accepted: {}",
            invalid.query
        );
    }

    assert!(serde_json::from_value::<WebSearchQuery>(json!({
        "query": "public terms",
        "count": 5,
        "unexpected": true
    }))
    .is_err());
    assert_eq!(
        serde_json::from_value::<WebSearchQuery>(json!({"query":"public terms"}))
            .unwrap()
            .count,
        5
    );
}

#[test]
fn result_urls_only_allow_public_http_urls() {
    assert!(safe_url("https://www.rust-lang.org/learn").is_some());
    assert!(safe_url("http://example.org/public/path").is_some());
    for url in [
        "file:///etc/passwd",
        "https://localhost/private",
        "https://127.0.0.1/private",
        "https://10.0.0.1/private",
        "https://100.64.0.1/private",
        "https://0.1.2.3/private",
        "https://192.0.2.1/private",
        "https://[::1]/private",
        "https://[fc00::1]/private",
        "https://router.internal/private",
        "https://localhost./private",
        "https://user:password@example.com/private",
    ] {
        assert!(safe_url(url).is_none(), "unexpectedly accepted: {url}");
    }
}

fn browser_result(results: Vec<Value>, count: u8) -> Value {
    let response: BrowserResponse = serde_json::from_value(json!({
        "status":"available", "truncated": false, "results":results,
    }))
    .unwrap();
    response
        .into_result(count)
        .unwrap()
        .into_tool_value()
        .unwrap()
}

#[test]
fn bounds_sanitizes_and_deduplicates_results_in_rank_order() {
    let make_hit =
        |title: &str, url: &str, snippet: &str| json!({"title":title,"url":url,"snippet":snippet});
    let result = browser_result(
        vec![
            make_hit("Private", "http://127.0.0.1/", ""),
            make_hit(
                "First\u{202e}",
                "https://example.org/first",
                "See C:\\Users\\alice\\private",
            ),
            make_hit("Duplicate", "https://example.org/first", ""),
            make_hit("Second", "https://example.org/second", ""),
            make_hit("Third", "https://example.org/third", ""),
        ],
        2,
    );
    let hits = result["results"].as_array().unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits[0],
        make_hit(
            "First",
            "https://example.org/first",
            "[PRIVATE_PATH_OMITTED]"
        )
    );
    assert_eq!(hits[1]["title"], "Second");
    assert_eq!(result["provider"], "bing");
    assert_eq!(result["untrusted"], true);
    assert_eq!(result["truncated"], true);
    let large = (0..10)
        .map(|i| {
            make_hit(
                &"游".repeat(300),
                &format!("https://example.org/{i}/{}", "x".repeat(1950)),
                &"戏".repeat(900),
            )
        })
        .collect();
    let result = browser_result(large, 10);
    assert!(serde_json::to_vec(&result).unwrap().len() <= MAX_TOOL_RESULT_BYTES);
    let hits = result["results"].as_array().unwrap();
    assert!(!hits.is_empty() && hits.len() < 10);
    for hit in hits {
        assert!(hit["title"].as_str().unwrap().len() <= MAX_TITLE_BYTES);
        assert!(hit["snippet"].as_str().unwrap().len() <= MAX_SNIPPET_BYTES);
    }
}

#[test]
fn only_available_responses_can_supply_hits() {
    for status in ["unavailable", "needs_configuration"] {
        let response: BrowserResponse = serde_json::from_value(json!({
            "status":status, "truncated":false,
            "results":[{"title":"Unexpected", "url":"https://example.org/", "snippet":""}],
        }))
        .unwrap();
        let value = response.into_result(5).unwrap().into_tool_value().unwrap();
        assert_eq!(value["status"], status);
        assert_eq!(value["results"], json!([]));
    }
    assert_eq!(browser_result(vec![], 5)["status"], "available");
}

fn fixture_command(script: &str) -> Command {
    let mut command = Command::new("node");
    command.args(["-e", script]);
    command
}

#[tokio::test]
async fn exchanges_utf8_json_with_the_browser_helper() {
    let command = fixture_command(
        r#"
        const readline = require('node:readline').createInterface({input:process.stdin});
        readline.once('line', line => {
            const {query, count} = JSON.parse(line);
            if (query !== '模组 & "wiki"' || count !== 2) process.exit(1);
            process.stdout.write(JSON.stringify({status:'available',truncated:false,results:[]}), () => process.exit(0));
        });"#,
    );
    let response = run_browser(
        command,
        &query("模组 & \"wiki\"", 2),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(response.status == WebSearchStatus::Available);
}

#[tokio::test]
async fn rejects_broken_or_oversized_browser_replies() {
    for script in [
        "process.stdin.once('data', () => { console.log('{}'); process.exit(0); })",
        "process.stdin.once('data', () => process.stdout.write('x'.repeat(300000), () => process.exit(0)))",
        "process.exit(1)",
    ] {
        let result = run_browser(
            fixture_command(script),
            &query("public game", 1),
            &CancellationToken::new(),
        ).await;
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn cancelling_a_running_search_closes_its_browser_session() {
    let dir = tempfile::tempdir().unwrap();
    let started = dir.path().join("started");
    let closed = dir.path().join("closed");
    let mut command = fixture_command(
        r#"
        const fs = require('node:fs');
        process.stdin.on('data', () => fs.writeFileSync(process.argv[1], 'ready'));
        process.stdin.on('end', () => fs.writeFileSync(process.argv[2], 'closed'));"#,
    );
    command.arg(&started).arg(&closed);
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let task =
        tokio::spawn(async move { run_browser(command, &query("public game", 1), &token).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !started.is_file() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    assert!(matches!(task.await.unwrap(), Err(AppError::Cancelled)));
    assert!(closed.is_file());
}

#[tokio::test]
#[ignore = "requires the prepared desktop runtime, an installed browser and live Bing access"]
async fn live_bing_search_uses_bundled_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = McpRuntime::bundled(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/mcp-runtime/runtime.zip"),
        temp.path().canonicalize().unwrap().join("runtime"),
    );
    let result = search(
        &runtime,
        &query("Minecraft official website", 3),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(
        result.status == WebSearchStatus::Available,
        "{}",
        result.message
    );
    assert!(!result.results.is_empty() && result.results.len() <= 3);
}
