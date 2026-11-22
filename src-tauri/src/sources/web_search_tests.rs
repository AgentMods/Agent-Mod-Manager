use super::*;
use serde_json::json;

fn query(value: impl Into<String>, count: usize) -> WebSearchQuery {
    WebSearchQuery {
        query: value.into(),
        count,
    }
}

#[test]
fn accepts_long_public_queries_and_keeps_private_query_guards() {
    query("x", 1).validate().unwrap();
    query("x".repeat(1024), 1000).validate().unwrap();
    query("界".repeat(900), 5).validate().unwrap();
    query("word ".repeat(150) + "word", 5).validate().unwrap();
    query(format!("https://docs.rs/{}", "a".repeat(2500)), 5)
        .validate()
        .unwrap();
    query("https://docs.rs/serde/latest/serde/", 5)
        .validate()
        .unwrap();
    query("find https://example.com/search?q=/guide/", 5)
        .validate()
        .unwrap();

    for invalid in [
        query("", 1),
        query("x".repeat(MAX_BROWSER_REQUEST), 5),
        query("word ".repeat(150) + " file:///private/data", 5),
        query(
            "word ".repeat(150) + " https://user:password@example.com/",
            5,
        ),
        query("x", 0),
        query("mod￾\u{202e}", 11),
        query("/Users/alice/private.txt", 5),
        query(r#"C:\Users\alice\private.txt"#, 5),
        query("file:///etc/passwd", 5),
        query("https://localhost/private", 5),
        query("site:https://localhost/private", 5),
        query("site:https://alice:secret@example.com/private", 5),
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

fn browser_response(results: Vec<Value>, next: Option<BingPosition>) -> BrowserResponse {
    serde_json::from_value(json!({
        "status":"available", "truncated": false, "results":results, "next":next,
    }))
    .unwrap()
}

fn make_hit(title: &str, url: &str, snippet: &str) -> Value {
    json!({"title":title,"url":url,"snippet":snippet})
}

fn cursor(value: &Value) -> Option<Uuid> {
    serde_json::from_value(value["next_cursor"].clone()).unwrap()
}

#[tokio::test]
async fn sanitizes_hits_and_keeps_remainder_with_a_bound_cursor() {
    let rows = vec![
        make_hit("Blocked", "https://localhost/private", ""),
        make_hit(
            " First\u{202e} ",
            "https://example.org/first",
            "From C:\\Users\\alice",
        ),
        make_hit("Duplicate", "https://example.org/first#duplicate", ""),
        make_hit("Second", "https://example.org/second", ""),
        make_hit("Third", "https://example.org/third", ""),
    ];
    let sessions = SearchSessions::default();
    let cancel = CancellationToken::new();
    let request = query("mod data", 2);
    let page = sessions
        .search_using(&request, None, &cancel, |position, _| {
            assert_eq!(position, BingPosition::START);
            std::future::ready(Ok(browser_response(rows.clone(), None)))
        })
        .await
        .unwrap();
    assert_eq!(page["provider"], "bing");
    assert_eq!(page["untrusted"], true);
    assert_eq!(page["truncated"], true);
    assert_eq!(
        page["results"],
        json!([
            make_hit(
                "First",
                "https://example.org/first",
                "[PRIVATE_PATH_OMITTED]"
            ),
            make_hit("Second", "https://example.org/second", ""),
        ])
    );
    assert_eq!(page["page_end"], "more_results");
    let next = cursor(&page).unwrap();
    let fetch_unexpected = |_, _| async { panic!("buffered rows must not be refetched") };
    assert!(matches!(
        sessions
            .search_using(
                &query("different", 99),
                Some(next),
                &cancel,
                fetch_unexpected
            )
            .await,
        Err(AppError::NotFound(_))
    ));
    let last = sessions
        .search_using(
            &query("mod data", 100),
            Some(next),
            &cancel,
            fetch_unexpected,
        )
        .await
        .unwrap();
    assert_eq!(
        last["results"],
        json!([make_hit("Third", "https://example.org/third", "")])
    );
    assert_eq!(last["page_end"], "bing_exhausted");
    assert_eq!(cursor(&last), None);
    assert!(matches!(
        sessions
            .search_using(&request, Some(next), &cancel, fetch_unexpected)
            .await,
        Err(AppError::NotFound(_))
    ));
}

#[tokio::test]
async fn delivers_large_results_through_to_site_end_without_duplicates() {
    let make = |index| {
        make_hit(
            &"游".repeat(170),
            &format!("https://example.org/{index}/{}", "x".repeat(1930)),
            &"戏".repeat(510),
        )
    };
    let sessions = SearchSessions::default();
    let cancel = CancellationToken::new();
    let request = query("public mod", 2000);
    let fetch = |position: BingPosition, _| async move {
        Ok(match position.first {
            1 => browser_response(
                (1..=24).map(make).collect(),
                Some(BingPosition {
                    first: 25,
                    row_offset: 0,
                }),
            ),
            25 => browser_response(
                (20..=43).map(make).collect(),
                Some(BingPosition {
                    first: 49,
                    row_offset: 0,
                }),
            ),
            49 => browser_response((42..=60).map(make).collect(), None),
            _ => panic!("unexpected position: {position:?}"),
        })
    };
    let mut next = None;
    let mut found = Vec::new();
    for page_index in 0..8 {
        let page = sessions
            .search_using(&request, next, &cancel, fetch)
            .await
            .unwrap();
        let wire_bytes = serde_json::to_vec(&page).unwrap().len();
        assert!(wire_bytes <= MAX_TOOL_PAGE_BYTES);
        let hits = page["results"].as_array().unwrap();
        if page_index == 0 {
            assert!(wire_bytes > 32 * 1024);
            assert!(hits.len() > 10);
        }
        found.extend(hits.iter().cloned());
        next = cursor(&page);
        if next.is_none() {
            assert_eq!(page["page_end"], "bing_exhausted");
            break;
        }
    }
    assert_eq!(found, (1..=60).map(make).collect::<Vec<_>>());
    assert_eq!(next, None);
}

#[tokio::test]
async fn crosses_bing_pages_and_does_not_treat_a_failed_page_as_exhausted() {
    let sessions = SearchSessions::default();
    let cancel = CancellationToken::new();
    let request = query("public terms", 30);
    let fetch = |position: BingPosition, _| async move {
        let base = position.first;
        Ok(if base == 21 {
            serde_json::from_value(json!({"status":"unavailable","truncated":false,"results":[]}))
                .unwrap()
        } else {
            browser_response(
                (base..base + 10)
                    .map(|i| make_hit("Result", &format!("https://example.org/{i}"), ""))
                    .collect(),
                Some(BingPosition {
                    first: base + 10,
                    row_offset: 0,
                }),
            )
        })
    };
    let page = sessions
        .search_using(&request, None, &cancel, fetch)
        .await
        .unwrap();
    assert_eq!(page["status"], "available");
    assert_eq!(page["results"].as_array().unwrap().len(), 20);
    assert_eq!(page["page_end"], "bing_unavailable");
    let final_page = sessions
        .search_using(&request, cursor(&page), &cancel, |position, _| async move {
            assert_eq!(
                position,
                BingPosition {
                    first: 21,
                    row_offset: 0
                }
            );
            Ok(browser_response(
                vec![make_hit("Last", "https://example.org/21", "")],
                None,
            ))
        })
        .await
        .unwrap();
    assert_eq!(final_page["results"].as_array().unwrap().len(), 1);
    assert_eq!(final_page["page_end"], "bing_exhausted");
    assert_eq!(cursor(&final_page), None);
}

#[tokio::test]
async fn unavailable_responses_cannot_supply_hits_and_invalid_positions_are_not_followed() {
    let sessions = SearchSessions::default();
    let cancel = CancellationToken::new();
    for (status, reason, expected) in [
        ("unavailable", None, "bing_unavailable"),
        ("unavailable", Some("query_rejected"), "query_rejected"),
        ("needs_configuration", None, "needs_configuration"),
    ] {
        let page = sessions
            .search_using(&query("public", 5), None, &cancel, |_, _| async move {
                Ok(serde_json::from_value(
                    json!({"status":status,"reason":reason,"truncated":false,
                "results":[make_hit("Unexpected","https://example.org", "")]}),
                )
                .unwrap())
            })
            .await
            .unwrap();
        assert_eq!(page["status"], status);
        assert_eq!(page["results"], json!([]));
        assert_eq!(page["page_end"], expected);
        assert_eq!(cursor(&page), None);
    }
    for next in [
        BingPosition::START,
        BingPosition {
            first: 1,
            row_offset: 99,
        },
        BingPosition {
            first: 2,
            row_offset: 1,
        },
    ] {
        let page = sessions
            .search_using(
                &query("public", 5),
                None,
                &cancel,
                |position, _| async move {
                    assert_eq!(position, BingPosition::START);
                    Ok(browser_response(
                        vec![make_hit("Valid", "https://example.org/", "")],
                        Some(next),
                    ))
                },
            )
            .await
            .unwrap();
        assert_eq!(page["results"].as_array().unwrap().len(), 1);
        assert_eq!(page["page_end"], "invalid_pagination");
        assert_eq!(cursor(&page), None);
    }
}

fn wire(query: &str, count: usize) -> BrowserRequest<'_> {
    BrowserRequest {
        query,
        count,
        position: BingPosition::START,
    }
}

#[tokio::test]
async fn cancels_an_active_continuation_and_invalidates_its_cursor() {
    let sessions = std::sync::Arc::new(SearchSessions::default());
    let cancel = CancellationToken::new();
    let request = query("mod data", 1);
    let first = sessions
        .search_using(&request, None, &cancel, |_, _| async {
            Ok(browser_response(
                vec![make_hit("First", "https://example.org/1", "")],
                Some(BingPosition {
                    first: 11,
                    row_offset: 0,
                }),
            ))
        })
        .await
        .unwrap();
    let next = cursor(&first);
    let (started, ready) = tokio::sync::oneshot::channel();
    let (closed, cleaned) = tokio::sync::oneshot::channel();
    let worker = {
        let sessions = sessions.clone();
        let cancel = cancel.clone();
        let request = request.clone();
        let mut started = Some(started);
        let mut closed = Some(closed);
        tokio::spawn(async move {
            sessions
                .search_using(&request, next, &cancel, |position, child| {
                    assert_eq!(position.first, 11);
                    let started = started.take().unwrap();
                    let closed = closed.take().unwrap();
                    async move {
                        started.send(()).unwrap();
                        child.cancelled().await;
                        closed.send(()).unwrap();
                        Err(AppError::Cancelled)
                    }
                })
                .await
        })
    };
    ready.await.unwrap();
    cancel.cancel();
    assert!(matches!(worker.await.unwrap(), Err(AppError::Cancelled)));
    cleaned.await.unwrap();
    assert!(matches!(
        sessions
            .search_using(&request, next, &CancellationToken::new(), |_, _| async {
                panic!("cancelled cursor must not be resumed")
            })
            .await,
        Err(AppError::NotFound(_))
    ));
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
        &wire("模组 & \"wiki\"", 2),
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
            &wire("public game", 1),
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
        tokio::spawn(async move { run_browser(command, &wire("public game", 1), &token).await });
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
