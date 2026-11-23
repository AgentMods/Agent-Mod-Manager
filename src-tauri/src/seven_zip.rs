use crate::error::{AppError, AppResult};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();
const CLI: &str = if cfg!(windows) { "7z.exe" } else { "7zz" };

#[cfg(not(test))]
pub(crate) fn set_resource_dir(resource_dir: PathBuf) {
    let _ = RESOURCE_DIR.set(resource_dir);
}

pub(crate) fn command() -> AppResult<Command> {
    if let Some(dir) = RESOURCE_DIR.get() {
        // Once Tauri has resolved the package resources, a broken installation must not
        // silently fall back to the workspace, a user-configured path, or the current directory.
        return command_in(dir);
    }
    #[cfg(test)]
    return command_in(&Path::new(env!("CARGO_MANIFEST_DIR")).join("resources"));
    #[cfg(not(test))]
    Err(AppError::Other("7-Zip 资源目录尚未初始化".to_string()))
}

fn command_in(resource_dir: &Path) -> AppResult<Command> {
    let dir = resource_dir.join("seven-zip");
    let program = dir.join(CLI);
    if !program.is_absolute()
        || !program.is_file()
        || (cfg!(windows) && !dir.join("7z.dll").is_file())
    {
        return Err(AppError::NotFound(
            "随应用分发的 7-Zip 不完整；请重新安装应用，开发或测试时请运行 yarn prepare:seven-zip"
                .to_string(),
        ));
    }
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_the_provisioned_cli() {
        let mut cmd = command().expect("run yarn prepare:seven-zip before running Rust tests");
        assert!(Path::new(cmd.get_program()).is_absolute());
        let output = cmd.arg("i").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let info = String::from_utf8_lossy(&output.stdout);
        assert!(info.contains("Igor Pavlov") && info.contains("Formats:"));
    }

    #[test]
    fn rejects_missing_and_relative_resources() {
        let empty = tempfile::tempdir().unwrap();
        assert!(matches!(
            command_in(empty.path()),
            Err(AppError::NotFound(_))
        ));
        assert!(matches!(
            command_in(Path::new("resources")),
            Err(AppError::NotFound(_))
        ));
    }
}
