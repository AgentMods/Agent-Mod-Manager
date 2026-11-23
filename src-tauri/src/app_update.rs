use serde::Serialize;
use std::fs::File;
use std::io::{self, Read, Seek, Write};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
#[cfg(not(test))]
use tauri::Manager;
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const CHECK_ERROR: &str = "无法检查应用更新，请稍后重试。";
const DOWNLOAD_ERROR: &str = "无法下载或验证应用更新，请稍后重试。";
const STAGE_ERROR: &str = "无法暂存应用更新，请检查磁盘空间后重试。";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub(crate) enum AppUpdateStatus {
    Idle,
    Checking,
    Downloading { version: String },
    Ready { version: String },
    UpToDate,
    Error { message: &'static str },
    Unsupported { message: &'static str },
}

pub(crate) struct StagedUpdate {
    update: Update,
    file: File,
}

impl StagedUpdate {
    fn read_payload(mut self) -> io::Result<(Update, Vec<u8>)> {
        let mut bytes = Vec::new();
        self.file.read_to_end(&mut bytes)?;
        Ok((self.update, bytes))
    }

    #[cfg(not(test))]
    pub(crate) fn install_at_exit(self) {
        let (update, bytes) = match self.read_payload() {
            Ok(payload) => payload,
            Err(error) => {
                tracing::warn!(%error, "could not read staged application update on exit");
                return;
            }
        };
        tracing::info!(version = %update.version, "installing staged application update on exit");
        // Wait for any in-progress filesystem transaction to commit or roll back,
        // and block new writers while the updater installs or hands off to Windows.
        let _writes = crate::transaction::write_lock();
        // install() re-verifies the bytes. On Windows it terminates this process
        // after launching the installer; on Linux/macOS it returns without relaunching.
        if let Err(error) = update.restart_after_install(false).install(bytes) {
            tracing::warn!(%error, "application update installation on exit failed");
        }
    }
}

fn write_staged_file(mut file: File, bytes: &[u8]) -> io::Result<File> {
    file.write_all(bytes)?;
    file.rewind()?;
    Ok(file)
}

enum Phase {
    Idle,
    Checking,
    Downloading { version: String },
    Ready(Box<StagedUpdate>),
    UpToDate,
    Error(&'static str),
    Unsupported(&'static str),
    Exiting,
}

impl Phase {
    fn status(&self) -> AppUpdateStatus {
        match self {
            Self::Idle | Self::Exiting => AppUpdateStatus::Idle,
            Self::Checking => AppUpdateStatus::Checking,
            Self::Downloading { version } => AppUpdateStatus::Downloading {
                version: version.clone(),
            },
            Self::Ready(staged) => AppUpdateStatus::Ready {
                version: staged.update.version.clone(),
            },
            Self::UpToDate => AppUpdateStatus::UpToDate,
            Self::Error(message) => AppUpdateStatus::Error { message },
            Self::Unsupported(message) => AppUpdateStatus::Unsupported { message },
        }
    }

    fn begin_check(&mut self) -> bool {
        if matches!(self, Self::Idle | Self::UpToDate | Self::Error(_)) {
            *self = Self::Checking;
            true
        } else {
            false
        }
    }

    fn finish_download(&mut self, result: Self) {
        // If exit already took the slot, dropping the result closes the file
        // without ever publishing it as installable.
        if matches!(self, Self::Downloading { .. }) {
            *self = result;
        }
    }

    fn take_at_exit(&mut self, normal_exit: bool) -> Option<Box<StagedUpdate>> {
        match std::mem::replace(self, Self::Exiting) {
            Self::Ready(staged) if normal_exit => Some(staged),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct AppUpdates(Arc<Mutex<Phase>>);

impl AppUpdates {
    fn new(phase: Phase) -> Self {
        Self(Arc::new(Mutex::new(phase)))
    }

    fn phase(&self) -> MutexGuard<'_, Phase> {
        self.0.lock().expect("application updater state poisoned")
    }

    // Claim the work before spawning. Startup and every IPC caller share this same slot.
    fn start_check<R: Runtime>(&self, app: &AppHandle<R>) -> AppUpdateStatus {
        let (start, status) = {
            let mut phase = self.phase();
            (phase.begin_check(), phase.status())
        };
        if start {
            let state = self.clone();
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let updater = app
                    .updater_builder()
                    .timeout(CHECK_TIMEOUT)
                    .configure_client(|client| {
                        client
                            .https_only(true)
                            .tls_danger_accept_invalid_certs(false)
                            .tls_danger_accept_invalid_hostnames(false)
                    })
                    .build();
                state.check_and_download(updater).await;
            });
        }
        status
    }

    async fn check_and_download(&self, updater: tauri_plugin_updater::Result<Updater>) {
        let result = async { updater?.check().await }.await;
        let mut update = {
            let mut phase = self.phase();
            if !matches!(*phase, Phase::Checking) {
                return;
            }
            match result {
                Ok(Some(update)) => {
                    *phase = Phase::Downloading {
                        version: update.version.clone(),
                    };
                    update
                }
                Ok(None) => {
                    *phase = Phase::UpToDate;
                    return;
                }
                Err(error) => {
                    tracing::warn!(%error, "application update check failed");
                    *phase = Phase::Error(CHECK_ERROR);
                    return;
                }
            }
        };

        update.timeout = Some(DOWNLOAD_TIMEOUT);
        // Tauri verifies the downloaded bytes before it returns them. A failed or
        // incomplete download never becomes installable, even if the app exits mid-check.
        let bytes = match update.download(|_, _| {}, || {}).await {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(%error, "application update download or signature verification failed");
                self.phase().finish_download(Phase::Error(DOWNLOAD_ERROR));
                return;
            }
        };
        if !matches!(*self.phase(), Phase::Downloading { .. }) {
            return;
        }

        // The plugin returns a Vec, but keep only an anonymous, rewound file in
        // Ready. The OS cleans it up when the handle closes, including on crashes.
        let result = match tokio::task::spawn_blocking(move || {
            write_staged_file(tempfile::tempfile()?, &bytes)
        })
        .await
        {
            Ok(Ok(file)) => Phase::Ready(Box::new(StagedUpdate { update, file })),
            Ok(Err(error)) => {
                tracing::warn!(%error, "could not write application update to temporary storage");
                Phase::Error(STAGE_ERROR)
            }
            Err(error) => {
                tracing::warn!(%error, "application update staging task failed");
                Phase::Error(STAGE_ERROR)
            }
        };
        self.phase().finish_download(result);
    }
}

#[tauri::command]
pub(crate) fn get_app_update_status(state: State<'_, AppUpdates>) -> AppUpdateStatus {
    state.phase().status()
}

#[tauri::command]
pub(crate) fn check_app_update<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppUpdates>,
) -> AppUpdateStatus {
    state.start_check(&app)
}

fn configured(config: Option<&serde_json::Value>) -> bool {
    config.is_some_and(|config| {
        config["pubkey"]
            .as_str()
            .is_some_and(|key| !key.trim().is_empty())
            && config["endpoints"]
                .as_array()
                .is_some_and(|endpoints| !endpoints.is_empty())
    })
}

fn manifest_target(
    platform: &str,
    bundle: Option<tauri::utils::config::BundleType>,
    has_appimage: bool,
    in_app_bundle: bool,
) -> Option<String> {
    use tauri::utils::config::BundleType;
    let (os, _) = platform.split_once('-')?;
    let installer = match (os, bundle?) {
        ("windows", BundleType::Nsis) => "nsis",
        ("windows", BundleType::Msi) => "msi",
        ("linux", BundleType::AppImage) if has_appimage => "appimage",
        ("linux", BundleType::Deb) => "deb",
        ("darwin", BundleType::App) if in_app_bundle => "app",
        _ => return None,
    };
    Some(format!("{platform}-{installer}"))
}

#[cfg(not(test))]
fn installed_target(app: &AppHandle) -> Option<String> {
    #[cfg(target_os = "linux")]
    let has_appimage = app.env().appimage.as_deref().is_some_and(|path| {
        let path = std::path::Path::new(path);
        path.is_absolute() && path.is_file()
    });
    #[cfg(not(target_os = "linux"))]
    let has_appimage = false;
    #[cfg(not(target_os = "linux"))]
    let _ = app;

    // Tauri reports `App` on macOS even for an unbundled executable. Do not let
    // the installer replace an arbitrary directory in that case.
    #[cfg(target_os = "macos")]
    let in_app_bundle = tauri::utils::platform::current_exe().is_ok_and(|path| {
        path.parent()
            .filter(|dir| dir.ends_with("Contents/MacOS"))
            .and_then(|dir| dir.parent()?.parent())
            .is_some_and(|dir| dir.extension().is_some_and(|extension| extension == "app"))
    });
    #[cfg(not(target_os = "macos"))]
    let in_app_bundle = false;

    manifest_target(
        &tauri_plugin_updater::target()?,
        tauri::utils::platform::bundle_type(),
        has_appimage,
        in_app_bundle,
    )
}

#[cfg(not(test))]
pub(crate) fn setup(app: &AppHandle) {
    let phase = if !configured(app.config().plugins.0.get("updater")) {
        Phase::Unsupported("此版本未配置签名更新。")
    } else if let Some(target) = installed_target(app) {
        // Require the exact installer key. An OS/arch-only fallback could switch
        // between MSI/NSIS or between Debian packages and AppImages.
        let plugin = tauri_plugin_updater::Builder::new()
            .target(target)
            .default_version_comparator(|current, update| update.version > current)
            .build();
        match app.plugin(plugin) {
            Ok(()) => Phase::Idle,
            Err(error) => {
                tracing::warn!(%error, "application updater initialization failed");
                Phase::Unsupported("此版本的更新配置不可用。")
            }
        }
    } else {
        Phase::Unsupported("此安装方式不支持自动更新。")
    };
    let state = AppUpdates::new(phase);
    app.manage(state.clone());
    state.start_check(app);
}

#[cfg(not(test))]
pub(crate) fn take_at_exit(app: &AppHandle, normal_exit: bool) -> Option<Box<StagedUpdate>> {
    let state = app.try_state::<AppUpdates>()?;
    let mut phase = state.phase();
    phase.take_at_exit(normal_exit)
}

#[cfg(test)]
mod tests;
