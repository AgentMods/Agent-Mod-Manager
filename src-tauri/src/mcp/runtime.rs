//! Application-owned MCP launchers. Bare aliases never fall back to a binary on PATH.
use crate::error::{AppError, AppResult};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, Seek},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

const PYPI_INDEX: &str = "https://pypi.tuna.tsinghua.edu.cn/simple";
const NPM_REGISTRY: &str = "https://registry.npmmirror.com";
const NPX_CLI: &str = "npm/bin/npx-cli.js";
const BING_HELPER: &str = "bin/bing-search.cjs";
const PLAYWRIGHT_PACKAGE: &str = "bin/node_modules/playwright-core/package.json";
const PLAYWRIGHT_CLI: &str = "bin/node_modules/playwright-core/cli.js";
const PLAYWRIGHT_ENTRY: &str = "bin/node_modules/playwright-core/index.js";
const NODE: &str = if cfg!(windows) { "node.exe" } else { "node" };
const UVX: &str = if cfg!(windows) { "uvx.exe" } else { "uvx" };
const UV: &str = if cfg!(windows) { "uv.exe" } else { "uv" };

#[derive(Clone, Copy, PartialEq, Eq)]
enum Launcher {
    Npx,
    Uvx,
}
impl Launcher {
    fn from_command(command: &str) -> Option<Self> {
        match command.to_ascii_lowercase().as_str() {
            "npx" | "npx.exe" | "npx.cmd" => Some(Self::Npx),
            "uvx" | "uvx.exe" => Some(Self::Uvx),
            _ => None,
        }
    }
}

/// Shared by all desktop MCP connections. Default supports HTTP and custom executables only.
#[derive(Clone, Default)]
pub struct McpRuntime(Option<Arc<Bundle>>);
struct Bundle {
    archive: PathBuf,
    cache: PathBuf,
    ready: Mutex<Option<PathBuf>>,
}
impl McpRuntime {
    pub fn bundled(archive: PathBuf, cache: PathBuf) -> Self {
        Self(Some(Arc::new(Bundle {
            archive,
            cache,
            ready: Mutex::new(None),
        })))
    }
    pub(super) async fn command(
        &self,
        executable: &str,
        cancel: &CancellationToken,
    ) -> AppResult<Command> {
        let path = std::env::var_os("PATH");
        let Some(launcher) = Launcher::from_command(executable) else {
            let mut cmd = background_command(executable);
            if let Some(path) = path {
                cmd.env("PATH", path);
            }
            return Ok(cmd);
        };
        let root = self.prepare_root(cancel, Bundle::root).await?;
        bundled_command(launcher, &root, path)
    }

    pub(crate) async fn bing_search_command(
        &self,
        cancel: &CancellationToken,
    ) -> AppResult<Command> {
        let root = self.prepare_root(cancel, Bundle::bing_root).await?;
        let mut cmd = browser_node(&root, BING_HELPER);
        cmd.current_dir(dunce::simplified(&root).join("bin"));
        Ok(cmd)
    }

    pub(super) async fn playwright_commands(
        &self,
        cancel: &CancellationToken,
    ) -> AppResult<(Command, Command)> {
        let root = self.prepare_root(cancel, Bundle::playwright_root).await?;
        // Use the same locked CLI and per-user Playwright cache for both processes.
        let mut install = browser_node(&root, PLAYWRIGHT_CLI);
        install.args([
            "install",
            "--no-shell",
            "--no-remove",
            "--no-progress",
            "chromium",
        ]);
        // Large CDN responses can take longer than Playwright's default socket timeout.
        install.env("PLAYWRIGHT_DOWNLOAD_CONNECTION_TIMEOUT", "120000");
        // Only the official downloader receives transport proxy settings.
        for key in [
            "HTTPS_PROXY",
            "HTTP_PROXY",
            "NO_PROXY",
            "ALL_PROXY",
            "https_proxy",
            "http_proxy",
            "no_proxy",
            "all_proxy",
        ] {
            if let Some(value) = std::env::var_os(key) {
                install.env(key, value);
            }
        }
        #[cfg(windows)]
        windows_installer_proxy(&mut install);
        let mut server = browser_node(&root, PLAYWRIGHT_CLI);
        server.arg("mcp");
        Ok((install, server))
    }

    async fn prepare_root(
        &self,
        cancel: &CancellationToken,
        prepare: fn(&Bundle) -> AppResult<PathBuf>,
    ) -> AppResult<PathBuf> {
        let bundle = self
            .0
            .clone()
            .ok_or_else(|| AppError::NotFound("未配置应用内置的 MCP 启动程序".into()))?;
        // Serialize cold preparation on a blocking worker. Cancellation may finish the cache,
        // but cannot start a child: the worker only returns a path.
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(AppError::Cancelled),
            result = tokio::task::spawn_blocking(move || prepare(&bundle)) => result
                .map_err(|_| AppError::Other("准备内置 MCP 运行时失败".into()))?,
        }
    }
}

// Browser discovery and OS integration only. Do not inherit NODE_*, proxy or
// service credentials, Playwright executable overrides, or MCP connection env.
#[cfg(windows)]
const BROWSER_ENV: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SystemRoot",
    "WINDIR",
    "SystemDrive",
    "COMSPEC",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "LOCALAPPDATA",
    "APPDATA",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "TEMP",
    "TMP",
];
#[cfg(not(windows))]
const BROWSER_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "TMPDIR",
    "TEMP",
    "TMP",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "XDG_RUNTIME_DIR",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
];

// Node's downloader reads proxy environment variables, unlike a Windows browser which
// also uses Internet Settings. Import only an enabled, static HTTPS proxy for setup.
#[cfg(windows)]
fn windows_installer_proxy(install: &mut Command) {
    if std::env::var_os("HTTPS_PROXY").is_some() || std::env::var_os("ALL_PROXY").is_some() {
        return;
    }
    let Ok(settings) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
    else {
        return;
    };
    if settings.get_value::<u32, _>("ProxyEnable").unwrap_or(0) == 0 {
        return;
    }
    let Ok(value) = settings.get_value::<String, _>("ProxyServer") else {
        return;
    };
    let proxy = if value.contains('=') {
        value
            .split(';')
            .filter_map(|part| part.split_once('='))
            .find_map(|(scheme, address)| {
                scheme
                    .trim()
                    .eq_ignore_ascii_case("https")
                    .then_some(address.trim())
            })
    } else {
        Some(value.trim())
    };
    let Some(address) = proxy.filter(|s| !s.is_empty()) else {
        return;
    };
    let url = if address.contains("://") {
        address.to_owned()
    } else {
        format!("http://{address}")
    };
    let Ok(url) = url::Url::parse(&url) else {
        return;
    };
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return;
    }
    install.env("HTTPS_PROXY", url.as_str());
    if std::env::var_os("NO_PROXY").is_none() {
        if let Ok(bypass) = settings.get_value::<String, _>("ProxyOverride") {
            install.env("NO_PROXY", bypass.replace(';', ","));
        }
    }
}

fn background_command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    cmd.env_clear();
    // MCP helpers communicate over pipes and can be restored before the first UI interaction.
    #[cfg(windows)]
    cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    cmd
}

fn browser_node(root: &Path, script: &str) -> Command {
    let root = dunce::simplified(root);
    let mut cmd = background_command(root.join("bin").join(NODE));
    cmd.arg("--no-global-search-paths")
        .arg(root.join(script))
        .kill_on_drop(true);
    for key in BROWSER_ENV {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    cmd
}

fn bundled_command(launcher: Launcher, root: &Path, path: Option<OsString>) -> AppResult<Command> {
    // npm package shims must find this same Node; avoid verbatim paths in Windows .cmd files.
    let root = dunce::simplified(root);
    let bin = root.join("bin");
    let path = std::env::join_paths(
        std::iter::once(bin.clone()).chain(path.iter().flat_map(std::env::split_paths)),
    )
    .map_err(|_| AppError::Invalid("内置运行时路径无法加入子进程 PATH".into()))?;
    let mut cmd = background_command(bin.join(if launcher == Launcher::Npx { NODE } else { UVX }));
    cmd.env("PATH", path);
    match launcher {
        Launcher::Npx => {
            cmd.arg(root.join(NPX_CLI))
                .env("npm_config_registry", NPM_REGISTRY);
        }
        Launcher::Uvx => {
            cmd.env("UV_DEFAULT_INDEX", PYPI_INDEX);
        }
    }
    Ok(cmd)
}
impl Bundle {
    fn playwright_root(&self) -> AppResult<PathBuf> {
        let root = self.root()?;
        verify_files([root.join("bin").join(NODE), root.join(PLAYWRIGHT_CLI)])?;
        Ok(root)
    }

    fn bing_root(&self) -> AppResult<PathBuf> {
        let root = self.root()?;
        // Keep Bing-only requirements out of the independent MCP launcher paths.
        verify_files([
            root.join("bin").join(NODE),
            root.join(BING_HELPER),
            root.join(PLAYWRIGHT_PACKAGE),
            root.join(PLAYWRIGHT_ENTRY),
        ])?;
        Ok(root)
    }

    fn root(&self) -> AppResult<PathBuf> {
        let mut ready = self.ready.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(root) = &*ready {
            return Ok(root.clone());
        }
        let mut input = File::open(&self.archive).map_err(|_| {
            AppError::NotFound(
                "缺少内置 MCP 运行时资源，请重新安装应用；从源码运行时请先准备运行时资源。".into(),
            )
        })?;
        let digest = crate::util::sha256_reader(&mut input)?;
        input.rewind()?;
        // Under the default non-relocatable private directory; inaccessible to Agent file tools.
        crate::encrypted_store::private_directory(&self.cache)?;
        let root = self.cache.join(digest);
        match fs::symlink_metadata(&root) {
            Ok(meta) if meta.is_dir() && root.canonicalize()? == root => verify_layout(&root)?,
            Ok(_) => return Err(AppError::Denied("内置运行时缓存被重定向".into())),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let stage = tempfile::Builder::new()
                    .prefix(".stage-")
                    .tempdir_in(&self.cache)?;
                extract(input, stage.path())?;
                verify_layout(stage.path())?;
                // Publish only a complete tree. The desktop has a single-instance lock.
                fs::rename(stage.path(), &root)?;
            }
            Err(err) => return Err(err.into()),
        }
        *ready = Some(root.clone());
        Ok(root)
    }
}

fn verify_layout(root: &Path) -> AppResult<()> {
    verify_files([
        root.join("bin").join(NODE),
        root.join("bin").join(UVX),
        root.join("bin").join(UV),
        root.join(NPX_CLI),
        root.join("npm/bin/npm-cli.js"),
    ])
}

fn verify_files(paths: impl IntoIterator<Item = PathBuf>) -> AppResult<()> {
    for path in paths {
        if !path.is_file() || path.canonicalize()? != path {
            return Err(AppError::Other(
                "内置 MCP 运行时不完整或被重定向，请检查运行时缓存与安装包。".into(),
            ));
        }
    }
    Ok(())
}
fn extract(input: File, root: &Path) -> AppResult<()> {
    let mut zip = zip::ZipArchive::new(input)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let path = crate::paths::safe_join(root, entry.name())?;
        if entry.is_symlink() {
            return Err(AppError::UnsafeArchive(
                "内置运行时不允许包含符号链接".into(),
            ));
        }
        if entry.is_dir() {
            fs::create_dir_all(path)?;
            continue;
        }
        fs::create_dir_all(path.parent().unwrap())?;
        let mut file = File::create_new(&path)?;
        io::copy(&mut entry, &mut file)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if path.parent() == Some(root.join("bin").as_path())
                && [NODE, UV, UVX].iter().any(|name| path.ends_with(name))
                || entry.unix_mode().unwrap_or(0) & 0o111 != 0
            {
                file.set_permissions(fs::Permissions::from_mode(0o700))?;
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, io::Write};
    fn archive(dir: &Path, extras: &[&str]) -> McpRuntime {
        let path = dir.join("fixture.zip");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        for name in [
            format!("bin/{NODE}"),
            format!("bin/{UVX}"),
            format!("bin/{UV}"),
            NPX_CLI.into(),
            "npm/bin/npm-cli.js".into(),
        ]
        .into_iter()
        .chain(extras.iter().map(|name| (*name).to_owned()))
        {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"test fixture").unwrap();
        }
        zip.finish().unwrap();
        McpRuntime::bundled(path, dir.join("cache"))
    }

    #[tokio::test]
    async fn bare_commands_use_packaged_launchers_path_and_only_their_own_mirror() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        let runtime = archive(&dir, &[]);
        let cancel = CancellationToken::new();
        for alias in ["npx", "NPX.CMD", "npx.exe", "uvx", "UVX.EXE"] {
            let cmd = runtime.command(alias, &cancel).await.unwrap();
            let cmd = cmd.as_std();
            let npx = alias.to_ascii_lowercase().starts_with("npx");
            let exe = Path::new(cmd.get_program());
            assert!(exe.is_absolute() && exe.is_file());
            assert_eq!(exe.file_name().unwrap(), if npx { NODE } else { UVX });
            let env: std::collections::HashMap<_, _> = cmd
                .get_envs()
                .map(|(k, v)| (k.to_str().unwrap().to_ascii_lowercase(), v.unwrap()))
                .collect();
            assert_eq!(env.len(), 2);
            assert_eq!(
                std::env::split_paths(env["path"]).next().unwrap(),
                exe.parent().unwrap()
            );
            if npx {
                assert!(Path::new(cmd.get_args().next().unwrap()).ends_with(NPX_CLI));
                assert_eq!(env["npm_config_registry"], NPM_REGISTRY);
            } else {
                assert_eq!(cmd.get_args().count(), 0);
                assert_eq!(env["uv_default_index"], PYPI_INDEX);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_ne!(fs::metadata(exe).unwrap().permissions().mode() & 0o100, 0);
            }
        }
        assert_eq!(dir.join("cache").read_dir().unwrap().count(), 1);
        // A fresh app instance reuses the complete digest-named cache.
        let reopened = McpRuntime::bundled(dir.join("fixture.zip"), dir.join("cache"));
        reopened.command("uvx", &cancel).await.unwrap();
        assert_eq!(dir.join("cache").read_dir().unwrap().count(), 1);
    }

    #[tokio::test]
    async fn explicit_commands_are_untouched_and_missing_bundles_never_fall_back() {
        let runtime = McpRuntime::default();
        let cancel = CancellationToken::new();
        for name in ["/opt/tools/npx", "./uvx", r"C:\custom\uvx.exe", "my-uvx"] {
            let cmd = runtime.command(name, &cancel).await.unwrap();
            let cmd = cmd.as_std();
            assert_eq!(cmd.get_program(), name);
            assert!(cmd
                .get_envs()
                .all(|(key, _)| key.eq_ignore_ascii_case("PATH")));
        }
        assert!(runtime.command("npx", &cancel).await.is_err());
        let dir = tempfile::tempdir().unwrap();
        let runtime = archive(&dir.path().canonicalize().unwrap(), &["../escape"]);
        assert!(runtime.command("uvx", &cancel).await.is_err());
        assert_eq!(dir.path().join("cache").read_dir().unwrap().count(), 0);
        assert!(!dir.path().join("escape").exists());
    }

    #[tokio::test]
    async fn playwright_installer_and_mcp_share_the_cli_but_not_proxy_credentials() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = archive(&tmp.path().canonicalize().unwrap(), &[PLAYWRIGHT_CLI]);
        let cancel = CancellationToken::new();
        let (install, server) = runtime.playwright_commands(&cancel).await.unwrap();
        let (install, server) = (install.as_std(), server.as_std());
        assert_eq!(install.get_program(), server.get_program());
        assert_eq!(
            install.get_args().take(2).collect::<Vec<_>>(),
            server.get_args().take(2).collect::<Vec<_>>()
        );
        assert!(install.get_args().any(|arg| arg == "install"));
        assert!(server.get_args().any(|arg| arg == "mcp"));
        assert!(!server.get_envs().any(|(key, _)| key
            .to_string_lossy()
            .to_ascii_uppercase()
            .ends_with("_PROXY")));
        cancel.cancel();
        assert!(matches!(
            runtime.playwright_commands(&cancel).await,
            Err(AppError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn bing_uses_only_its_bundled_node_and_browser_environment() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        let runtime = archive(&dir, &[BING_HELPER, PLAYWRIGHT_PACKAGE, PLAYWRIGHT_ENTRY]);
        let cancel = CancellationToken::new();
        let cmd = runtime.bing_search_command(&cancel).await.unwrap();
        let cmd = cmd.as_std();
        let bin = Path::new(cmd.get_program()).parent().unwrap();
        assert!(bin.is_absolute());
        assert_eq!(Path::new(cmd.get_program()), bin.join(NODE));
        assert_eq!(cmd.get_current_dir(), Some(bin));
        let args = cmd.get_args().collect::<Vec<_>>();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "--no-global-search-paths");
        assert_eq!(Path::new(args[1]), bin.join("bing-search.cjs"));
        assert!(bin
            .join("node_modules/playwright-core/package.json")
            .is_file());
        let actual = cmd.get_envs().collect::<HashMap<_, _>>();
        let expected = BROWSER_ENV
            .iter()
            .filter_map(|key| std::env::var_os(key).map(|val| (*key, val)))
            .collect::<HashMap<_, _>>();
        assert_eq!(actual.len(), expected.len());
        for (key, value) in actual {
            let name = key.to_str().unwrap();
            let expected_value = expected
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_os_str());
            assert!(value.is_some());
            assert_eq!(value, expected_value);
            assert!(![
                "NODE_OPTIONS",
                "NODE_PATH",
                "AMM_BING_BROWSER_PATH",
                "OPENAI_API_KEY",
                "HTTPS_PROXY"
            ]
            .iter()
            .any(|key| key.eq_ignore_ascii_case(name)));
        }
        cancel.cancel();
        assert!(matches!(
            runtime.bing_search_command(&cancel).await,
            Err(AppError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn missing_or_redirected_bing_files_do_not_break_npx_or_uvx() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        let cancel = CancellationToken::new();
        let legacy = archive(&dir, &[]);
        assert!(legacy.bing_search_command(&cancel).await.is_err());
        legacy.command("npx", &cancel).await.unwrap();

        let runtime = archive(&dir, &[BING_HELPER, PLAYWRIGHT_PACKAGE, PLAYWRIGHT_ENTRY]);
        let cmd = runtime.bing_search_command(&cancel).await.unwrap();
        let bin = Path::new(cmd.as_std().get_program()).parent().unwrap();
        let root = bin.parent().unwrap();
        let entry = root.join(PLAYWRIGHT_ENTRY);
        let reopened = McpRuntime::bundled(dir.join("fixture.zip"), dir.join("cache"));
        reopened.bing_search_command(&cancel).await.unwrap();

        fs::remove_file(&entry).unwrap();
        assert!(reopened.bing_search_command(&cancel).await.is_err());
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join(BING_HELPER), &entry).unwrap();
        #[cfg(windows)]
        fs::create_dir(&entry).unwrap();
        assert!(runtime.bing_search_command(&cancel).await.is_err());
        runtime.command("uvx", &cancel).await.unwrap();
    }
}
