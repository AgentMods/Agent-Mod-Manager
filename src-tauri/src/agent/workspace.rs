//! Workspace is default context, not filesystem authorization.
use crate::{
    db::Db,
    error::{AppError, AppResult},
    history::History,
};
pub(super) fn selection(db: &Db, conversation: &str) -> AppResult<Option<Vec<String>>> {
    History::new(db.clone()).conversation_workspace(conversation)
}
pub(crate) fn resolve(ids: Option<&[String]>, requested: Option<&str>) -> AppResult<String> {
    if let Some(id) = requested {
        if ids.is_none_or(|ids| ids.iter().any(|candidate| candidate == id)) {
            return Ok(id.into());
        }
    } else if let Some([id]) = ids {
        return Ok(id.clone());
    }
    Err(AppError::Invalid(
        "请明确提供当前工作区中的目标游戏 ID".into(),
    ))
}
pub(super) fn plugin_visible(ids: Option<&[String]>, adapters: &[String], name: &str) -> bool {
    ids.is_none() || adapters.iter().any(|adapter| adapter == name)
}
/// Global scope exposes only a registered id/name index; explicit scopes include target details.
pub(super) fn prompt_games(db: &Db, ids: Option<&[String]>) -> AppResult<Vec<serde_json::Value>> {
    let Some(ids) = ids else {
        let conn = db.conn();
        let mut statement = conn.prepare("SELECT id,name FROM games ORDER BY name,id")?;
        let rows = statement.query_map([], |row| {
            Ok(serde_json::json!({"id": row.get::<_, String>(0)?, "name": row.get::<_, String>(1)?}))
        })?;
        return rows.collect::<Result<Vec<_>, _>>().map_err(Into::into);
    };
    ids.iter()
        .map(|id| {
            let game = crate::games::get_game(db, id)?;
            let brief = serde_json::json!({
                "id": game.id, "name": game.name, "slug": game.slug,
                "rootPath": crate::paths::display_path(std::path::Path::new(&game.root_path)),
                "platform": game.platform, "adapterSlug": game.adapter_slug,
                "adapterVersion": game.adapter_version, "steamAppId": game.steam_app_id
            });
            Ok(brief)
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prompt_games_are_explicit_and_compact() {
        let (_temp, engine, _) = crate::transaction::tests::setup();
        let index = prompt_games(&engine.db, None).unwrap();
        assert!(!index.is_empty());
        assert!(index.iter().any(|game| game["id"] == "g"));
        for game in &index {
            let fields = game.as_object().unwrap();
            assert_eq!(fields.len(), 2);
            assert!(fields["id"].is_string());
            assert!(fields["name"].is_string());
        }
        assert!(prompt_games(&engine.db, Some(&[])).unwrap().is_empty());
        let games = prompt_games(&engine.db, Some(&["g".into()])).unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0]["id"], "g");
        assert!(games[0]["rootPath"].is_string());
        assert!(games[0]["platform"].is_string());
        for field in [
            "metadata",
            "coverPath",
            "installedModCount",
            "enabledModCount",
            "createdAt",
            "updatedAt",
        ] {
            assert!(games[0].get(field).is_none(), "unexpected {field}");
        }
    }
    #[test]
    fn targets_preserve_all_empty_single_and_multiple() {
        let ids = vec!["a".into(), "b".into()];
        assert!(resolve(None, None).is_err()); // all must never become a single default
        assert_eq!(resolve(None, Some("other")).unwrap(), "other");
        assert!(resolve(Some(&[]), None).is_err());
        assert!(resolve(Some(&[]), Some("a")).is_err());
        assert_eq!(resolve(Some(&ids[..1]), None).unwrap(), "a");
        assert!(resolve(Some(&ids[..1]), Some("b")).is_err());
        assert!(resolve(Some(&ids), None).is_err());
        assert_eq!(resolve(Some(&ids), Some("b")).unwrap(), "b");
        assert!(resolve(Some(&ids), Some("other")).is_err());
    }
    #[test]
    fn global_includes_unbound_plugin_guides_but_explicit_empty_does_not() {
        assert!(plugin_visible(None, &[], "unbound-plugin"));
        assert!(!plugin_visible(Some(&[]), &[], "unbound-plugin"));
        let ids = vec!["a".into()];
        let adapters = vec!["selected-adapter".into()];
        assert!(plugin_visible(Some(&ids), &adapters, "selected-adapter"));
        assert!(!plugin_visible(Some(&ids), &adapters, "other"));
    }
}
