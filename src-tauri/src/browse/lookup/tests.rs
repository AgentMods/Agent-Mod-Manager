use super::*;
use crate::browse::tests::{game, game_db, provider_game};
use std::cell::{Cell, RefCell};

#[derive(Default)]
struct Catalog {
    rows: BTreeMap<&'static str, Vec<(&'static str, &'static str)>>,
    aliases: Vec<String>,
    selected_label: Option<&'static str>,
    queries: RefCell<Vec<String>>,
    alias_calls: Cell<usize>,
    selection_calls: Cell<usize>,
    truncated: bool,
    broad: Option<&'static str>,
    rejected_term: Option<&'static str>,
}

impl CatalogLookup for Catalog {
    async fn page(
        &self,
        query: &str,
        _cursor: Option<&str>,
    ) -> Result<ProviderGamePage, IssueCode> {
        self.queries.borrow_mut().push(query.to_owned());
        if self.rejected_term == Some(query) {
            return Err(IssueCode::PermissionDenied);
        }
        let games = self
            .rows
            .get(query)
            .into_iter()
            .flatten()
            .map(|(id, title)| provider_game(id, title))
            .collect();
        Ok(ProviderGamePage {
            games,
            next: (self.truncated || self.broad == Some(query)).then(|| "1".to_owned()),
        })
    }
    async fn aliases(&self, _: &str, _: Option<&str>) -> Result<Vec<String>, IssueCode> {
        self.alias_calls.set(self.alias_calls.get() + 1);
        Ok(self.aliases.clone())
    }
    async fn suggest(
        &self,
        _: &str,
        _: Option<&str>,
        labels: &[String],
    ) -> Result<Option<usize>, IssueCode> {
        self.selection_calls.set(self.selection_calls.get() + 1);
        Ok(labels
            .iter()
            .position(|label| Some(label.as_str()) == self.selected_label))
    }
}

fn registered(name: &str) -> GameScope {
    let dir = tempfile::tempdir().unwrap();
    let db = game_db(&dir.path().join("games.db"));
    let mut game = game(&db, "game-a");
    game.name = name.to_owned();
    game
}

#[tokio::test]
async fn trusted_title_and_remembered_match_shortcut_model_calls() {
    let game = registered("影之刃零");
    let steam = "Phantom Blade Zero";
    let catalog = Catalog {
        rows: BTreeMap::from([(steam, vec![("a", steam)])]),
        ..Default::default()
    };
    let result = resolve(&catalog, &game, &game.name, Some(steam), "nexusmods", None)
        .await
        .unwrap();
    assert!(matches!(result, Match::Exact(id) if id == "a"));
    assert_eq!(*catalog.queries.borrow(), [steam]);
    assert_eq!(catalog.alias_calls.get(), 0);
    assert_eq!(catalog.selection_calls.get(), 0);

    let catalog = Catalog {
        rows: BTreeMap::from([("影之刃零", vec![("b", "Other label")])]),
        ..Default::default()
    };
    let result = resolve(
        &catalog,
        &game,
        &game.name,
        Some(steam),
        "glossmod_http",
        Some("b"),
    )
    .await
    .unwrap();
    assert!(matches!(result, Match::Exact(id) if id == "b"));
    assert_eq!(catalog.alias_calls.get(), 0);
    assert_eq!(catalog.selection_calls.get(), 0);
}

#[tokio::test]
async fn local_spelling_can_rediscover_a_preferred_manual_choice_after_a_steam_exact_hit() {
    let game = registered("本地名称");
    let steam = "Storefront Title";
    let catalog = Catalog {
        rows: BTreeMap::from([
            (steam, vec![("exact", steam)]),
            ("本地名称", vec![("user-choice", "User selected edition")]),
        ]),
        ..Default::default()
    };
    let result = resolve(
        &catalog,
        &game,
        &game.name,
        Some(steam),
        "nexusmods",
        Some("user-choice"),
    )
    .await
    .unwrap();
    assert!(matches!(result, Match::Exact(id) if id == "user-choice"));
    assert_eq!(catalog.alias_calls.get(), 0);
}

#[tokio::test]
async fn discover_by_alias_then_resolve_each_edition_from_live_rows() {
    let base = "GTA V";
    let legacy = "Grand Theft Auto V (Legacy)";
    let enhanced = "Grand Theft Auto V (Enhanced)";
    for (local, label, expected_id) in [
        ("GTA5传承版", legacy, "34"),
        ("GTA5增强版", enhanced, "221"),
    ] {
        let game = registered(local);
        let catalog = Catalog {
            rows: BTreeMap::from([(base, vec![("221", enhanced), ("34", legacy)])]),
            aliases: vec![local.into(), base.into(), base.to_lowercase()],
            selected_label: Some(label),
            ..Default::default()
        };
        let result = resolve(&catalog, &game, local, None, "glossmod_3dm", None)
            .await
            .unwrap();
        assert!(matches!(result, Match::Exact(id) if id == expected_id));
        assert_eq!(*catalog.queries.borrow(), [local, base, "GTA5"]);
        assert_eq!(catalog.selection_calls.get(), 1);
    }
}

#[tokio::test]
async fn no_model_or_edition_evidence_preserves_manual_choices() {
    let game = registered("GTA5增强版");
    let base = "Grand Theft Auto V";
    let legacy = "Grand Theft Auto V (Legacy)";
    for suggestion in [None, Some(base), Some(legacy)] {
        let catalog = Catalog {
            rows: BTreeMap::from([(base, vec![("generic", base), ("wrong", legacy)])]),
            selected_label: suggestion,
            ..Default::default()
        };
        let result = resolve(&catalog, &game, &game.name, Some(base), "nexusmods", None)
            .await
            .unwrap();
        assert!(matches!(result, Match::Choices(choices) if choices.len() == 2));
    }
}

#[tokio::test]
async fn broad_names_find_manual_candidates_without_a_model_or_steam() {
    let catalog = Catalog {
        rows: BTreeMap::from([("GTA5", vec![("grand-theft-auto-v", "GTA5")])]),
        ..Default::default()
    };
    let game = registered("GTA5传承版");
    let result = resolve(&catalog, &game, &game.name, None, "glossmod_http", None)
        .await
        .unwrap();
    assert!(
        matches!(result, Match::Choices(choices) if choices.len() == 1 && choices[0].name == "GTA5")
    );
    assert_eq!(*catalog.queries.borrow(), ["GTA5传承版", "GTA5"]);
}

#[tokio::test]
async fn contradictory_editions_require_user_selection() {
    let game = registered("GTA5增强版");
    let catalog = Catalog {
        rows: BTreeMap::from([(
            "GTA5",
            vec![("both", "Grand Theft Auto V Enhanced / Legacy")],
        )]),
        selected_label: Some("Grand Theft Auto V Enhanced / Legacy"),
        ..Default::default()
    };
    for steam in [None, Some("Grand Theft Auto V Legacy")] {
        let result = resolve(&catalog, &game, &game.name, steam, "glossmod_http", None)
            .await
            .unwrap();
        assert!(matches!(result, Match::Choices(_)));
    }
}

#[tokio::test]
async fn bounded_or_revoked_catalog_cannot_be_resolved_by_the_model() {
    let game = registered("Alias");
    let catalog = Catalog {
        truncated: true,
        rows: BTreeMap::from([("Alias", vec![("id", "Spelling")])]),
        selected_label: Some("Spelling"),
        ..Default::default()
    };
    let result = resolve(&catalog, &game, &game.name, None, "gamebanana", None)
        .await
        .unwrap();
    assert!(matches!(result, Match::Choices(choices) if choices.len()==1));
    assert_eq!(catalog.selection_calls.get(), 0);
    let revoked = Catalog {
        aliases: vec!["New name".into()],
        truncated: false,
        rejected_term: Some("New name"),
        ..catalog
    };
    assert!(matches!(
        resolve(&revoked, &game, &game.name, None, "gamebanana", None).await,
        Err(IssueCode::PermissionDenied)
    ));
}

#[tokio::test]
async fn a_truncated_query_still_allows_a_narrow_alias_but_never_hides_a_known_duplicate() {
    let game = registered("简称");
    for (broad_label, ambiguous) in [("Another game", false), ("Specific spelling", true)] {
        let catalog = Catalog {
            broad: Some("简称"),
            rows: BTreeMap::from([
                ("简称", vec![("old", broad_label)]),
                ("Full Title", vec![("new", "Specific spelling")]),
            ]),
            aliases: vec!["Full Title".into()],
            selected_label: Some("Specific spelling"),
            ..Default::default()
        };
        let result = resolve(&catalog, &game, &game.name, None, "gamebanana", None)
            .await
            .unwrap();
        if ambiguous {
            assert!(matches!(result, Match::Choices(options) if options.len() == 2));
        } else {
            assert!(matches!(result, Match::Exact(id) if id == "new"));
        }
    }
}
