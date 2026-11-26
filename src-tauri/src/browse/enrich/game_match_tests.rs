use super::*;
use crate::{error::AppResult, settings::SecretStore};
use rusqlite::Connection;
use serde_json::Value;
use std::sync::{Arc, Mutex};

fn memory_db() -> Db {
    Db(Arc::new(Mutex::new(Connection::open_in_memory().unwrap())))
}

fn titles(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[tokio::test]
async fn aliases_preserve_gta_edition_priority_and_exclude_repeated_input() {
    let db = memory_db();
    let cancel = CancellationToken::new();
    let aliases = aliases_with(
        &db,
        "GTA5增强版",
        Some("Grand Theft Auto V Enhanced"),
        &cancel,
        |payload| async move {
            assert_eq!(
                serde_json::from_str::<Value>(&payload).unwrap(),
                json!({
                    "name":"GTA5增强版", "steamTitle":"Grand Theft Auto V Enhanced"
                })
            );
            Some(
                json!({
                    "officialTitles":["侠盗猎车手 V 增强版", " grand theft auto v ENHANCED "],
                    "abbreviations":["GTAV Enhanced"],
                    "baseTitle":"Grand Theft Auto V"
                })
                .to_string(),
            )
        },
    )
    .await;
    assert_eq!(
        aliases,
        ["侠盗猎车手 V 增强版", "GTAV Enhanced", "Grand Theft Auto V"]
    );

    let aliases = aliases_with(
        &db,
        "GTA5传承版",
        Some("Grand Theft Auto V Legacy"),
        &cancel,
        |_| async {
            Some(
                json!({
                    "officialTitles":["侠盗猎车手 V 传承版"],
                    "abbreviations":["侠盗猎车手  V  传承版"],
                    "baseTitle":"GTA V"
                })
                .to_string(),
            )
        },
    )
    .await;
    assert_eq!(aliases, ["侠盗猎车手 V 传承版", "GTA V"]);
}

#[tokio::test]
async fn malformed_and_oversize_aliases_are_never_used_or_cached() {
    let db = memory_db();
    let cancel = CancellationToken::new();
    let fixed = r#"{"officialTitles":["星露谷物语"],"abbreviations":[],"baseTitle":null}"#;
    for reply in [
        r#"[["星露谷物语"],[],null]"#.into(),
        r#"{"officialTitles":[],"abbreviations":[]}"#.into(),
        r#"{"officialTitles":[],"abbreviations":[],"baseTitle":null,"id":"stardew"}"#.into(),
        r#"{"officialTitles":["星露谷物语","Stardew","Star Dew"],"abbreviations":[],"baseTitle":null}"#.into(),
        r#"{"officialTitles":[],"abbreviations":["SV","SDV"],"baseTitle":null}"#.into(),
        r#"{"officialTitles":["星露谷物语","https://other.example/title"],"abbreviations":[],"baseTitle":null}"#.into(),
        r#"{"officialTitles":["www.other.example"],"abbreviations":[],"baseTitle":null}"#.into(),
        r#"{"officialTitles":["C:\\Games\\Stardew"],"abbreviations":[],"baseTitle":null}"#.into(),
        r#"{"officialTitles":["Stardew\nValley","<title>"],"abbreviations":[],"baseTitle":null}"#.into(),
        r#"{"officialTitles":["Stardew\u202eValley"],"abbreviations":[],"baseTitle":null}"#.into(),
        r#"{"officialTitles":[" 1234 "],"abbreviations":[],"baseTitle":null}"#.into(),
        json!({"officialTitles":["星".repeat(MAX_ALIAS_BYTES / 3 + 1)],"abbreviations":[],"baseTitle":null}).to_string(),
        format!("{fixed}{}", " ".repeat(MAX_GAME_REPLY_BYTES)),
    ] {
        assert!(aliases_with(&db, "Stardew Valley", None, &cancel, |_| async {Some(reply)}).await.is_empty());
    }
    assert_eq!(
        aliases_with(&db, "Stardew Valley", None, &cancel, |_| async {
            Some(fixed.into())
        })
        .await,
        ["星露谷物语"]
    );
}

#[tokio::test]
async fn invalid_input_and_candidate_bounds_precede_model_requests() {
    let db = memory_db();
    let cancel = CancellationToken::new();
    let unexpected = |_| async { panic!("invalid input reached the model") };
    let oversize_name = "A".repeat(super::super::MAX_QUERY_BYTES + 1);
    for name in [
        " ",
        r"C:\Users\Player",
        "app@example.test",
        "游戏\n目录",
        &oversize_name,
    ] {
        assert!(aliases_with(&db, name, None, &cancel, unexpected)
            .await
            .is_empty());
    }
    assert!(
        aliases_with(&db, "GTA V", Some("file:///GTA"), &cancel, unexpected)
            .await
            .is_empty()
    );
    let too_many = vec!["Game".into(); MAX_GAME_CANDIDATES + 1];
    for options in [
        &[][..],
        &titles(&["GTA V", "https://other.example"]),
        &too_many,
    ] {
        assert_eq!(
            suggest_with(&db, "GTA V", None, "3DM", options, &cancel, unexpected).await,
            None
        );
    }
    assert_eq!(
        suggest_with(
            &db,
            "GTA V",
            None,
            "data:private",
            &titles(&["GTA V"]),
            &cancel,
            unexpected
        )
        .await,
        None
    );

    let mut options = (1..MAX_GAME_CANDIDATES)
        .map(|index| format!("Other Game {index}"))
        .collect::<Vec<_>>();
    options.push("GTA V Enhanced".into());
    assert_eq!(
        suggest_with(
            &db,
            "GTA V Enhanced",
            None,
            "mod.io",
            &options,
            &cancel,
            |_| async { Some(json!({"index":MAX_GAME_CANDIDATES - 1}).to_string()) }
        )
        .await,
        Some(MAX_GAME_CANDIDATES - 1)
    );
}

#[tokio::test]
async fn gta_suggestions_keep_zero_based_order_and_model_abstentions() {
    let db = memory_db();
    let cancel = CancellationToken::new();
    let options = titles(&[
        "GTA V Enhanced",
        "Grand Theft Auto V",
        "GTA V Legacy",
        "GTA Online",
    ]);
    for (name, steam, response, expected) in [
        (
            "GTA5",
            Some("Grand Theft Auto V Enhanced"),
            r#"{"index":0}"#,
            Some(0),
        ),
        ("GTA5传承版", None, r#"{"index":2}"#, Some(2)),
        ("GTA5", None, r#"{"index":null}"#, None),
        (
            "GTA5传承版",
            Some("Grand Theft Auto V Enhanced"),
            r#"{"index":null}"#,
            None,
        ),
    ] {
        assert_eq!(
            suggest_with(
                &db,
                name,
                steam,
                "Nexus Mods",
                &options,
                &cancel,
                |payload| {
                    assert_eq!(
                        serde_json::from_str::<Value>(&payload).unwrap(),
                        json!({
                            "name":name,"steamTitle":steam,"source":"Nexus Mods","options":options
                        })
                    );
                    async move { Some(response.into()) }
                }
            )
            .await,
            expected
        );
    }
    for options in [
        titles(&["Grand Theft Auto V"]),
        titles(&["GTA V Legacy"]),
        titles(&["GTA Online"]),
    ] {
        assert_eq!(
            suggest_with(
                &db,
                "GTA5增强版",
                None,
                "3DM",
                &options,
                &cancel,
                |_| async { Some(r#"{"index":null}"#.into()) }
            )
            .await,
            None
        );
    }
}

#[tokio::test]
async fn suggestion_rejects_duplicate_titles_indexes_and_invented_ids() {
    let db = memory_db();
    let cancel = CancellationToken::new();
    let options = titles(&["GTA V Enhanced", "GTA V Legacy"]);
    let oversize = format!("{{\"index\":0}}{}", " ".repeat(MAX_GAME_REPLY_BYTES));
    for reply in [
        "{}",
        "[0]",
        r#"{"index":null}"#,
        r#"{"index":-1}"#,
        r#"{"index":0.0}"#,
        r#"{"index":"0"}"#,
        r#"{"index":[0,1]}"#,
        r#"{"index":2}"#,
        r#"{"index":18446744073709551616}"#,
        r#"{"index":0,"index":1}"#,
        r#"{"index":0,"id":"gta5-enhanced"}"#,
        &oversize,
    ] {
        assert_eq!(
            suggest_with(
                &db,
                "GTA V Enhanced",
                None,
                "3DM",
                &options,
                &cancel,
                |_| async { Some(reply.into()) }
            )
            .await,
            None
        );
    }
    let identical = titles(&["GTA V Enhanced", "gta  v  enhanced"]);
    assert_eq!(
        suggest_with(
            &db,
            "GTA V Enhanced",
            None,
            "3DM",
            &identical,
            &cancel,
            |_| async { Some(r#"{"index":0}"#.into()) }
        )
        .await,
        None
    );
    assert_eq!(
        suggest_with(
            &db,
            "GTA V Enhanced",
            None,
            "3DM",
            &options,
            &cancel,
            |_| async { Some(r#"{"index":0}"#.into()) }
        )
        .await,
        Some(0)
    );
}

#[tokio::test]
async fn cache_is_durable_and_selection_is_scoped_by_steam_source_and_ordered_titles() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("browse.sqlite");
    let db = Db::open(&path).unwrap();
    let cancel = CancellationToken::new();
    let steam = Some("Grand Theft Auto V Enhanced");
    let options = titles(&["GTA V Enhanced", "GTA V Legacy"]);
    let alias = |_| async {
        Some(r#"{"officialTitles":[],"abbreviations":["GTA V Enhanced"],"baseTitle":null}"#.into())
    };
    let select = |_| async { Some(r#"{"index":0}"#.into()) };
    assert_eq!(
        aliases_with(&db, "GTA5", steam, &cancel, alias).await,
        ["GTA V Enhanced"]
    );
    assert_eq!(
        suggest_with(&db, "GTA5", steam, "3DM", &options, &cancel, select).await,
        Some(0)
    );
    drop(db);

    let db = Db::open(&path).unwrap();
    let cache_hit = |_| async { panic!("unexpected model request on a cache hit") };
    assert_eq!(
        aliases_with(&db, "GTA5", steam, &cancel, cache_hit).await,
        ["GTA V Enhanced"]
    );
    assert_eq!(
        suggest_with(&db, "GTA5", steam, "3DM", &options, &cancel, cache_hit).await,
        Some(0)
    );
    let cache_miss = |_| async { None };
    assert!(aliases_with(&db, "GTA5", None, &cancel, cache_miss)
        .await
        .is_empty());
    assert!(aliases_with(&db, "Other Name", steam, &cancel, cache_miss)
        .await
        .is_empty());
    assert_eq!(
        suggest_with(&db, "GTA5", None, "3DM", &options, &cancel, cache_miss).await,
        None
    );
    assert_eq!(
        suggest_with(
            &db,
            "Other Name",
            steam,
            "3DM",
            &options,
            &cancel,
            cache_miss
        )
        .await,
        None
    );
    assert_eq!(
        suggest_with(
            &db,
            "GTA5",
            steam,
            "Nexus Mods",
            &options,
            &cancel,
            cache_miss
        )
        .await,
        None
    );
    let reversed = options.into_iter().rev().collect::<Vec<_>>();
    assert_eq!(
        suggest_with(&db, "GTA5", steam, "3DM", &reversed, &cancel, cache_miss).await,
        None
    );
}

#[tokio::test]
async fn cancelled_requests_and_missing_model_yield_no_new_results() {
    struct NoSecrets;
    impl SecretStore for NoSecrets {
        fn read(&self, _: &str) -> AppResult<Option<String>> {
            panic!("no configured model needs a secret")
        }
        fn write(&self, _: &str, _: &str) -> AppResult<()> {
            panic!("browse model requests do not write secrets")
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let db = Db::open(&directory.path().join("browse.sqlite")).unwrap();
    let settings = Settings::with_secret_store(db, Arc::new(NoSecrets));
    let cancel = CancellationToken::new();
    let options = titles(&["Grand Theft Auto V Enhanced"]);
    let valid_alias = r#"{"officialTitles":[],"abbreviations":["GTAV Enhanced"],"baseTitle":null}"#;
    assert!(game_aliases(&settings, "GTA5增强版", None, &cancel)
        .await
        .is_empty());
    assert_eq!(
        suggest_game(&settings, "GTA5增强版", None, "3DM", &options, &cancel).await,
        None
    );
    assert!(
        aliases_with(&settings.db, "GTA5增强版", None, &cancel, |_| async {
            cancel.cancel();
            Some(valid_alias.into())
        })
        .await
        .is_empty()
    );
    assert!(aliases_with(
        &settings.db,
        "GTA5增强版",
        None,
        &CancellationToken::new(),
        |_| async { None }
    )
    .await
    .is_empty());
    assert_eq!(
        suggest_with(
            &settings.db,
            "GTA5增强版",
            None,
            "3DM",
            &options,
            &cancel,
            |_| async { panic!("cancelled request reached the model") }
        )
        .await,
        None
    );
}
