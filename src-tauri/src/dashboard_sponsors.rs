use crate::error::AppResult;
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
struct Sponsor {
    name: String,
}

/// The public wall includes payment metadata. Only display names cross into the webview.
#[tauri::command]
pub async fn list_dashboard_sponsors() -> AppResult<Vec<String>> {
    let mut response = reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .redirect(reqwest::redirect::Policy::none())
        .build()?
        .get("https://sponsor.aoe.top/api/list")
        .send()
        .await?
        .error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > 256 * 1024 - bytes.len() {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidData).into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let mut names = Vec::new();
    for sponsor in serde_json::from_slice::<Vec<Sponsor>>(&bytes)? {
        let name = sponsor.name.trim();
        if !name.is_empty() && !names.iter().any(|item| item == name) {
            names.push(name.to_owned());
            if names.len() == 6 {
                break;
            }
        }
    }
    Ok(names)
}
