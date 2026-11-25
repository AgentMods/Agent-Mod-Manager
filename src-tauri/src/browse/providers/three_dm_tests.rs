use super::*;

fn catalog_page(page: u64, page_size: u64, count: u64, data: Vec<Value>) -> Value {
    json!({
        "success": true,
        "msg": "success",
        "data": {
            "data": data,
            "count": count,
            "page": page,
            "pageSize": page_size,
            "totalPages": count.div_ceil(page_size),
        }
    })
}

#[test]
fn browse_three_dm_game_lookup_uses_the_english_and_localized_catalog_names() {
    let minecraft = json!({"id":9, "game_name":"我的世界", "game_ename":"Minecraft"});
    let cube_world = json!({"id":94, "game_name":"魔方世界", "game_ename":"Cube World"});
    let game = Request::new(Source::ThreeDm, Operation::Games, None, "Minecraft", None).unwrap();
    assert_eq!(
        game.args,
        json!({"page":1,"pageSize":20,"search":"Minecraft"})
    );
    let payload = catalog_page(1, 20, 2, vec![minecraft, cube_world]);
    let (rows, next) = game.rows(&payload).unwrap();
    assert_eq!(next, None);
    let matching = parse_game(Source::ThreeDm, &rows[0], "Minecraft").unwrap();
    assert_eq!(matching.id, "9");
    assert_eq!(matching.name, "Minecraft");
    assert_eq!(
        parse_game(Source::ThreeDm, &rows[0], "我的世界")
            .unwrap()
            .name,
        "我的世界"
    );
    assert_ne!(
        parse_game(Source::ThreeDm, &rows[1], "Minecraft")
            .unwrap()
            .name,
        "Minecraft"
    );
    let unfiltered = Request::new(Source::ThreeDm, Operation::Games, None, "", None).unwrap();
    assert_eq!(unfiltered.args, json!({"page":1,"pageSize":20}));
}

#[test]
fn browse_three_dm_feed_keyword_and_pagination_use_observed_wire_contract() {
    let feed = Request::new(Source::ThreeDm, Operation::Browse, Some("1"), "  ", None).unwrap();
    assert_eq!(feed.args, json!({"gameId":1,"page":1,"pageSize":20}));
    let first = Request::new(
        Source::ThreeDm,
        Operation::Search,
        Some("1"),
        " SKSE ",
        None,
    )
    .unwrap();
    assert_eq!(
        first.args,
        json!({"gameId":1,"page":1,"pageSize":20,"search":"SKSE"})
    );
    let payload = catalog_page(1, 20, 24, vec![Value::Null; 20]);
    let (rows, next) = first.rows(&payload).unwrap();
    assert_eq!(rows.len(), 20);
    assert_eq!(next.as_deref(), Some("2"));
    let second = Request::new(
        Source::ThreeDm,
        Operation::Search,
        Some("1"),
        "SKSE",
        next.as_deref(),
    )
    .unwrap();
    assert_eq!(
        second.args,
        json!({"gameId":1,"page":2,"pageSize":20,"search":"SKSE"})
    );
    let payload = catalog_page(2, 20, 24, vec![Value::Null; 4]);
    assert_eq!(second.rows(&payload).unwrap().1, None);
    let empty = catalog_page(1, 20, 0, vec![]);
    assert_eq!(first.rows(&empty).unwrap(), (&[][..], None));
    assert!(second.rows(&empty).is_err());
    assert!(first.rows(&catalog_page(1, 10, 24, vec![])).is_err());
    assert!(first
        .rows(&catalog_page(1, 20, 24, vec![Value::Null; 21]))
        .is_err());
    assert!(Request::new(Source::ThreeDm, Operation::Browse, Some("0"), "", None).is_err());

    let last = Request::new(
        Source::ThreeDm,
        Operation::Browse,
        Some("1"),
        "",
        Some("1000"),
    )
    .unwrap();
    let payload = catalog_page(1000, 20, 20_093, vec![Value::Null; 20]);
    assert_eq!(last.rows(&payload).unwrap().1, None);
    assert!(Request::new(
        Source::ThreeDm,
        Operation::Browse,
        Some("1"),
        "",
        Some("1001")
    )
    .is_err());
    assert!(Request::new(Source::ThreeDm, Operation::Games, None, "", Some("1001")).is_err());
}

#[test]
fn browse_three_dm_projects_check_scope_and_accept_first_party_webp_variants() {
    let mut row = json!({
        "id": 42, "game_id": 475, "mods_state": 3, "mods_title": "Work",
        "mods_author": "Original author", "user_nickName": "Uploader", "mods_desc": "",
        "mods_key": ["Visual"], "mods_type_name": "Game fixes",
        "mods_image_url": "/static/upload/mod/202611/MOD42@webp",
        "mods_original_url": "https://unapproved.invalid/elsewhere/1",
    });
    let item = parse_item(Source::ThreeDm, &row, "475").unwrap();
    assert_eq!(item.id, "42");
    assert_eq!(item.author.as_deref(), Some("Original author"));
    assert_eq!(item.summary, None);
    assert_eq!(item.kind.as_deref(), Some("Game fixes"));
    assert_eq!(item.tags, ["Visual"]);
    assert_eq!(
        item.page_url.as_deref(),
        Some("https://mod.3dmgame.com/mod/42")
    );
    assert_eq!(
        item.cover_url.as_deref(),
        Some("https://mod.3dmgame.com/static/upload/mod/202611/MOD42@webp")
    );
    assert!(parse_item(Source::ThreeDm, &row, "261").is_none());
    row.as_object_mut().unwrap().remove("game_id");
    assert!(parse_item(Source::ThreeDm, &row, "475").is_none());
    assert!(cover(
        Source::ThreeDm,
        Some("/static/upload/mod/202611/MOD42.png@webp")
    )
    .is_some());
    for bad in [
        "https://unapproved.invalid/static/upload/mod/202611/MOD42@webp",
        "/static/upload/resource/MOD42@webp",
        "/static/upload/mod/202611/MOD42@webp?token=fixture",
        "/static/upload/mod/../private/MOD42@webp",
    ] {
        assert!(cover(Source::ThreeDm, Some(bad)).is_none());
    }
}
