//! Application-owned MCP launchers. Bare aliases never fall back to a binary on PATH.
use crate::error::{AppError, AppResult};
use std::{
    ffi::OsString,
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
            let mut cmd = Command::new(executable);
            cmd.env_clear();
            if let Some(path) = path {
                cmd.env("PATH", path);
            }
            return Ok(cmd);
        };
        let bundle = self
            .0
            .clone()
            .ok_or_else(|| AppError::NotFound("未配置应用内置的 MCP 启动程序".into()))?;
        // Serialize cold preparation on a blocking worker. Cancellation may finish the cache,
        // but cannot start a child: the worker only returns a path.
        let root = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
            result = tokio::task::spawn_blocking(move || bundle.root()) => result
                .map_err(|_| AppError::Other("准备内置 MCP 运行时失败".into()))??,
        };
        bundled_command(launcher, &root, path)
    }
}
fn bundled_command(launcher: Launcher, root: &Path, path: Option<OsString>) -> AppResult<Command> {
    // npm package shims must find this same Node; avoid verbatim paths in Windows .cmd files.
    let root = dunce::simplified(root);
    let bin = root.join("bin");
    let path = std::env::join_paths(
        std::iter::once(bin.clone()).chain(path.iter().flat_map(std::env::split_paths)),
    )
    .map_err(|_| AppError::Invalid("内置运行时路径无法加入子进程 PATH".into()))?;
    let mut cmd = Command::new(bin.join(if launcher == Launcher::Npx { NODE } else { UVX }));
    cmd.env_clear().env("PATH", path);
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
    for path in [
        root.join("bin").join(NODE),
        root.join("bin").join(UVX),
        root.join("bin").join(UV),
        root.join(NPX_CLI),
        root.join("npm/bin/npm-cli.js"),
    ] {
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
            if path.starts_with(root.join("bin")) || entry.unix_mode().unwrap_or(0) & 0o111 != 0 {
                file.set_permissions(fs::Permissions::from_mode(0o700))?;
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn archive(dir: &Path, extra: Option<&str>) -> McpRuntime {
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
        .chain(extra.map(str::to_owned))
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
        let runtime = archive(&dir, None);
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
        let runtime = archive(&dir.path().canonicalize().unwrap(), Some("../escape"));
        assert!(runtime.command("uvx", &cancel).await.is_err());
        assert_eq!(dir.path().join("cache").read_dir().unwrap().count(), 0);
        assert!(!dir.path().join("escape").exists());
    }
}
