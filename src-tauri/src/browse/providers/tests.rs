use super::*;

fn fixture(source: Source) -> (Value, Value) {
    match source {
        Source::GlossMod => (
            json!({"items":[{"id":99,"slug":"inzoi","name":"inZOI（云族裔）","aliases":["云族裔"],"active":true}]}),
            json!({"items":[{"id":243,"title":"Work","creator_name":"Author","summary":"Public","status":"published","cover_url":"https://r2.glossmod.com/works/1/cover.jpg","tags":["test"]},{"id":244,"title":"private draft","status":"draft"}]}),
        ),
        Source::ThreeDm => (
            json!({"success":true,"data":{"data":[{"id":1,"game_name":"上古卷轴5：天际","game_ename":"The Elder Scrolls V:Skyrim"}],"page":1,"totalPages":3}}),
            json!({"success":true,"data":{"data":[{"id":12,"game_id":1,"mods_title":"Work","mods_author":"Author","mods_desc":"Public","mods_type_name":"工具","mods_image_url":"/static/upload/logo/202601/test.png","mods_key":["test"],"mods_resource":[{"mods_resource_url":"https://invalid.example/would-download.zip"}]},{"id":13,"game_id":2,"mods_title":"Unrelated"}],"page":1,"totalPages":3}}),
        ),
        Source::Nexus => (
            json!({"nodes":[{"domainName":"skyrim","name":"Skyrim"}], "totalCount":2}),
            json!({"nodes":[{"modId":8,"name":"Work","author":"Author","summary":"Public","pictureUrl":"https://staticdelivery.nexusmods.com/cover.png","game":{"domainName":"skyrim"}},{"modId":8,"name":"Other game","game":{"domainName":"skyrimspecialedition"}}],"totalCount":3}),
        ),
        Source::CurseForge => (
            json!({"data":[{"id":432,"slug":"minecraft","name":"Minecraft"}],"nextIndex":50,"nextPageSize":50}),
            json!({"data":[{"gameId":432,"id":70,"slug":"example","name":"Work","summary":"Public","authors":[{"name":"Author"}],"logo":{"url":"https://media.forgecdn.net/example.png"},"links":{"websiteUrl":"https://www.curseforge.com/minecraft/mc-mods/example","sourceUrl":"https://invalid.example"},"categories":[{"name":"Magic"}]}],"nextIndex":20}),
        ),
        Source::GameBanana => (
            json!({"_aMetadata":{"_bIsComplete":false},"_aRecords":[{"_idRow":4,"_sName":"Baldi"}]}),
            json!({"_aMetadata":{"_bIsComplete":false},"_aRecords":[{"_idRow":101,"_idGameRow":4,"_sModelName":"Mod","_sName":"Work","_sDescription":"Public","_aSubmitter":{"_sName":"Author"},"_aPreviewMedia":{"_aImages":[{"_sFile220":"small.jpg","_sBaseUrl":"https://images.gamebanana.com/img/ss/mods"}]}}, {"_idRow":102,"_sModelName":"Sound","_sName":"Not a Mod"}]}),
        ),
        Source::ModIo => (
            json!({"data":[{"id":17,"name":"RPG X","name_id":"rpg-x","status":1}],"next_offset":20}),
            json!({"data":[{"game_id":17,"id":9,"name":"Work","name_id":"work","status":1,"visible":1,"profile_url":"https://mod.io/g/rpg-x/m/work","summary":"Public","submitted_by":{"username":"Author"},"logo":{"thumb_320x180":"https://images.modcdn.io/thumbnail.png"},"tags":[{"name":"test"}]}, {"game_id":17,"id":10,"name":"Hidden","visible":0}],"next_offset":20}),
        ),
        Source::Thunderstore => (
            json!({"results":[{"identifier":"valheim","name":"Valheim"}],"next_offset":20}),
            json!({"results":[{"name":"Work","namespace":"Author","description":"Public","icon_url":"https://ccdn.thunderstore.io/live/cover.png","categories":[{"name":"Tools"}]}],"next":"https://untrusted.example/?ticket=private-untouched"}),
        ),
    }
}

#[test]
fn browse_sources_seven_game_search_response_and_continuation_contracts() {
    let expected = [
        (
            Source::GlossMod,
            "云族裔",
            "inzoi",
            "game",
            "243",
            None,
            None,
            "https://www.glossmod.com/mods/243",
        ),
        (
            Source::ThreeDm,
            "The Elder Scrolls V:Skyrim",
            "1",
            "gameId",
            "12",
            Some("2"),
            Some("2"),
            "https://mod.3dmgame.com/mod/12",
        ),
        (
            Source::Nexus,
            "Skyrim",
            "skyrim",
            "game_domain",
            "skyrim/8",
            Some("1"),
            Some("2"),
            "https://www.nexusmods.com/skyrim/mods/8",
        ),
        (
            Source::CurseForge,
            "Minecraft",
            "432",
            "game_id",
            "70",
            Some("50"),
            Some("20"),
            "https://www.curseforge.com/minecraft/mc-mods/example",
        ),
        (
            Source::GameBanana,
            "Baldi",
            "4",
            "game_id",
            "101",
            Some("2"),
            Some("2"),
            "https://gamebanana.com/mods/101",
        ),
        (
            Source::ModIo,
            "RPG X",
            "17",
            "game_id",
            "9",
            Some("20"),
            Some("20"),
            "https://mod.io/g/rpg-x/m/work",
        ),
        (
            Source::Thunderstore,
            "Valheim",
            "valheim",
            "community",
            "Author/Work",
            Some("20"),
            Some("2"),
            "https://thunderstore.io/c/valheim/p/Author/Work/",
        ),
    ];
    for (source, query, game, game_key, id, game_next, next, page_url) in expected {
        let (games, mods) = fixture(source);
        let req = Request::new(source, Operation::Games, None, query, None).unwrap();
        let (rows, token) = req.rows(&games).unwrap();
        assert_eq!(token.as_deref(), game_next, "{source:?}");
        assert_eq!(
            parse_game(source, &rows[0], query).unwrap(),
            ProviderGame {
                id: game.into(),
                name: query.into()
            }
        );
        let req = Request::new(source, Operation::Search, Some(game), "Work", None).unwrap();
        assert_eq!(
            req.args[game_key],
            number_string(game).map_or(json!(game), |id| json!(id))
        );
        let (rows, token) = req.rows(&mods).unwrap();
        assert_eq!(token.as_deref(), next, "{source:?}");
        let items: Vec<_> = rows
            .iter()
            .filter_map(|v| parse_item(source, v, game))
            .collect();
        assert_eq!(items.len(), 1, "{source:?}");
        let item = &items[0];
        assert_eq!(item.id, id);
        assert_eq!(item.title, "Work");
        assert_eq!(item.author.as_deref(), Some("Author"));
        assert_eq!(item.summary.as_deref(), Some("Public"));
        assert!(item.cover_url.is_some(), "{source:?}");
        assert_eq!(item.page_url.as_deref(), Some(page_url));
        assert!(
            Request::new(source, Operation::Browse, None, "", None).is_err(),
            "{source:?}"
        );
        assert!(Request::new(
            source,
            Operation::Search,
            Some(game),
            "x",
            Some("https://x/?ticket=2")
        )
        .is_err());
    }
}

#[test]
fn browse_never_guesses_game_or_exposes_invalid_links_or_optional_fields() {
    let duplicate = json!({"nodes":[{"domainName":"skyrim","name":"Skyrim"},{"domainName":"skyrimspecialedition","name":"Skyrim"}]});
    let req = Request::new(Source::Nexus, Operation::Games, None, "Skyrim", None).unwrap();
    let mut duplicate = duplicate;
    duplicate["totalCount"] = json!(2);
    let (rows, _) = req.rows(&duplicate).unwrap();
    assert_eq!(
        rows.iter()
            .filter_map(|r| parse_game(Source::Nexus, r, "Skyrim"))
            .count(),
        2
    );
    assert!(parse_game(Source::Nexus, &json!({"id":2,"name":"Skyrim"}), "Skyrim").is_none());
    assert!(parse_game(
        Source::GlossMod,
        &json!({"slug":"../admin","name":"Skyrim"}),
        "Skyrim"
    )
    .is_none());
    let item = parse_item(Source::CurseForge, &json!({
        "id":70,"name":"Work","slug":"example","links":{"websiteUrl":"https://www.curseforge.com/minecraft/mc-mods/different"},
        "logo":{"url":"https://media.forgecdn.net/image.png?token=private"}
    }), "432").unwrap();
    assert_eq!(item.author, None);
    assert_eq!(item.cover_url, None);
    assert_eq!(item.page_url, None);
    assert!(!item.title.is_empty());
    assert!(parse_item(
        Source::ModIo,
        &json!({"id":12,"name":"Unpublished","visible":0}),
        "17"
    )
    .is_none());
    assert!(cover(
        Source::ThreeDm,
        Some("/static/upload/logo/../private/token.jpg")
    )
    .is_none());
    assert!(strict_url(Some("https://mod.io@evil.invalid/g/x/m/y")).is_none());
    assert!(strict_url(Some("https://mod.io/g/x/m/%65vil")).is_none());
    assert!(parse_item(
        Source::Nexus,
        &json!({"modId":9,"name":"x","game":{"domainName":"different"}}),
        "skyrim"
    )
    .is_none());
}

#[test]
fn browse_distinguishes_empty_filtered_pages_from_bad_cursors() {
    let curse = Request::new(Source::CurseForge, Operation::Games, None, "x", None).unwrap();
    assert_eq!(
        curse
            .rows(&json!({"data":[],"nextIndex":50}))
            .unwrap()
            .1
            .as_deref(),
        Some("50")
    );
    assert!(curse.rows(&json!({"data":[],"nextIndex":0})).is_err());
    let banana = Request::new(Source::GameBanana, Operation::Games, None, "x", None).unwrap();
    assert_eq!(
        banana
            .rows(&json!({"_aRecords": [], "_aMetadata": {"_bIsComplete":false}}))
            .unwrap()
            .1
            .as_deref(),
        Some("2")
    );
    let gloss = Request::new(Source::GlossMod, Operation::Browse, Some("inzoi"), "", None).unwrap();
    assert_eq!(gloss.args["scope"], "");
    assert!(gloss.args.get("keyword").is_none());
    assert_eq!(
        gloss
            .rows(&json!({"items":vec![Value::Null;20]}))
            .unwrap()
            .1
            .as_deref(),
        Some("20")
    );
    assert!(gloss.rows(&json!({"different":[]})).is_err());
    assert!(Request::new(Source::ThreeDm, Operation::Browse, Some("1"), "", Some("0")).is_err());
    assert!(
        Request::new(Source::GameBanana, Operation::Browse, Some("1"), "", None)
            .unwrap()
            .args
            .get("query")
            .is_none()
    );
}

#[test]
fn browse_disconnected_impostor_and_precancelled_requests_never_open_transport() {
    let dir = tempfile::tempdir().unwrap();
    let servers = McpServers::new(crate::db::Db::open(&dir.path().join("provider.db")).unwrap());
    servers.seed_presets().unwrap();
    let connections = McpConnections::default();
    let statuses = sources(&connections, &servers);
    assert_eq!(statuses.len(), 7);
    assert!(statuses
        .into_iter()
        .all(|s| s.availability == ProviderAvailability::NotConnected && s.connection.is_none()));
    let mut impostor = servers.get("glossmod_http").unwrap();
    impostor.url = Some("https://example.invalid/mcp".into());
    servers.update(&impostor).unwrap();
    assert!(matches!(
        connection(&connections, &servers, "glossmod_http"),
        Err(AppError::Unsupported(_))
    ));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let identity = test_connection();
    assert!(identity != test_connection());
    assert!(matches!(
        bound_reader(&connections, &servers, Source::GlossMod, &identity, &cancel),
        Err(AppError::Cancelled)
    ));
}
