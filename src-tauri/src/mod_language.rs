use crate::{app::AppServices, error::AppResult, settings};
use serde::Serialize;
use tauri::State;

#[derive(Serialize)]
pub struct ModOrganizationLocale {
    locale: Option<String>,
    effective_locale: String,
    languages: &'static [(&'static str, &'static str)],
}

#[tauri::command]
pub fn get_mod_organization_locale(
    state: State<'_, AppServices>,
) -> AppResult<ModOrganizationLocale> {
    let conn = state.engine.db.conn();
    let locale = settings::mod_organization_locale_override(&conn)?;
    Ok(ModOrganizationLocale {
        effective_locale: locale.clone().unwrap_or_else(settings::system_mod_locale),
        locale,
        languages: settings::MOD_ORGANIZATION_LANGUAGES,
    })
}

#[tauri::command]
pub fn set_mod_organization_locale(
    state: State<'_, AppServices>,
    locale: Option<String>,
) -> AppResult<()> {
    settings::set_mod_organization_locale_override(&state.engine.db, locale.as_deref())
}
