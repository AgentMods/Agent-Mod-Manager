use crate::{app::AppServices, app_locale, error::AppResult, settings};
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
    let app_locale::AppLocale {
        locale,
        effective_locale,
    } = app_locale::get(&state.engine.db)?;
    Ok(ModOrganizationLocale {
        effective_locale,
        locale,
        languages: app_locale::LANGUAGES,
    })
}

#[tauri::command]
pub fn set_mod_organization_locale(
    state: State<'_, AppServices>,
    locale: Option<String>,
) -> AppResult<()> {
    settings::set_mod_organization_locale_override(&state.engine.db, locale.as_deref())
}
