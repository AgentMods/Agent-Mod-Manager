//! Filesystem primitives for paths already authorized and canonicalized by the caller.
//! On desktop platforms, resolve each component relative to a held directory.

use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
#[cfg(not(windows))]
use std::path::Component;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileIdentity {
    volume: u64,
    object: [u8; 16],
    birth: Option<(i64, u32)>,
}

fn wrong_identity(path: &Path) -> AppError {
    AppError::Conflict(format!("文件对象已改变: {}", path.display()))
}

#[cfg(not(windows))]
fn relative_components(path: &Path) -> AppResult<std::path::Components<'_>> {
    let parts = path.components();
    if !parts
        .clone()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(AppError::Denied("暂存条目必须是无穿越的相对路径".into()));
    }
    Ok(parts)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod platform {
    use super::{relative_components, wrong_identity, FileIdentity};
    use crate::error::{AppError, AppResult};
    use rustix::{
        fs::{self, AtFlags, FileType, Mode, OFlags, RenameFlags, CWD},
        io::Errno,
    };
    use std::{
        ffi::OsStr,
        fs::File,
        os::fd::OwnedFd,
        path::{Component, Components, Path, PathBuf},
    };

    const DIRECTORY: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);

    fn io(errno: Errno) -> AppError {
        std::io::Error::from(errno).into()
    }

    fn invalid_path(path: &Path) -> AppError {
        AppError::Denied(format!(
            "操作路径必须是无链接的绝对路径: {}",
            path.display()
        ))
    }

    fn directory(path: &Path, create: bool) -> AppResult<OwnedFd> {
        let mut components = path.components();
        if components.next() != Some(Component::RootDir)
            || !components
                .clone()
                .all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(invalid_path(path));
        }
        let root = fs::openat(CWD, "/", DIRECTORY, Mode::empty()).map_err(io)?;
        descend(root, components, create)
    }

    fn descend(mut dir: OwnedFd, components: Components<'_>, create: bool) -> AppResult<OwnedFd> {
        for component in components {
            let name = component.as_os_str();
            dir = match fs::openat(&dir, name, DIRECTORY, Mode::empty()) {
                Err(Errno::NOENT) if create => {
                    match fs::mkdirat(&dir, name, Mode::from_raw_mode(0o777)) {
                        Ok(()) | Err(Errno::EXIST) => {}
                        Err(error) => return Err(io(error)),
                    }
                    fs::openat(&dir, name, DIRECTORY, Mode::empty()).map_err(io)?
                }
                result => result.map_err(io)?,
            };
        }
        Ok(dir)
    }

    fn parent(path: &Path, create: bool) -> AppResult<(OwnedFd, &OsStr)> {
        let name = path.file_name().ok_or_else(|| invalid_path(path))?;
        let base = path.parent().ok_or_else(|| invalid_path(path))?;
        Ok((directory(base, create)?, name))
    }

    pub(crate) fn create_dir_all(path: &Path) -> AppResult<()> {
        directory(path, true)?;
        Ok(())
    }

    pub(crate) fn create_dir(path: &Path) -> AppResult<()> {
        let (dir, name) = parent(path, true)?;
        fs::mkdirat(&dir, name, Mode::from_raw_mode(0o777)).map_err(io)
    }

    pub(crate) fn create_new(path: &Path) -> AppResult<File> {
        let (dir, name) = parent(path, true)?;
        create_new_at(&dir, name)
    }

    fn create_new_at(dir: &OwnedFd, name: &OsStr) -> AppResult<File> {
        let fd = fs::openat(
            dir,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o666),
        )
        .map_err(io)?;
        Ok(fd.into())
    }

    pub(crate) fn open_regular(path: &Path) -> AppResult<File> {
        let (dir, name) = parent(path, false)?;
        let before = fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io)?;
        if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile {
            return Err(AppError::Denied("只允许打开普通文件".into()));
        }
        // NONBLOCK keeps a concurrent swap to a FIFO from hanging the process.
        let fd = fs::openat(
            dir,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io)?;
        let opened = fs::fstat(&fd).map_err(io)?;
        if FileType::from_raw_mode(opened.st_mode) != FileType::RegularFile
            || before.st_dev != opened.st_dev
            || before.st_ino != opened.st_ino
        {
            return Err(AppError::Conflict("文件在打开期间被替换".into()));
        }
        Ok(fd.into())
    }

    pub(crate) fn verify_regular_at(file: &File, path: &Path) -> AppResult<()> {
        let opened = fs::fstat(file).map_err(io)?;
        let (dir, name) = parent(path, false)?;
        let current = fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io)?;
        if FileType::from_raw_mode(opened.st_mode) != FileType::RegularFile
            || FileType::from_raw_mode(current.st_mode) != FileType::RegularFile
            || opened.st_nlink != 1
            || current.st_nlink != 1
            || opened.st_dev != current.st_dev
            || opened.st_ino != current.st_ino
        {
            return Err(wrong_identity(path));
        }
        Ok(())
    }

    pub(crate) fn directory_entries(
        path: &Path,
        expected: &FileIdentity,
    ) -> AppResult<impl Iterator<Item = AppResult<(std::ffi::OsString, bool)>>> {
        use std::os::unix::ffi::OsStrExt;

        let dir = directory(path, false)?;
        if identity_at(&dir, OsStr::new("."))?.as_ref() != Some(expected) {
            return Err(wrong_identity(path));
        }
        let entries = fs::Dir::read_from(&dir).map_err(io)?;
        Ok(entries.filter_map(|entry| {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => return Some(Err(io(err))),
            };
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                return None;
            }
            Some(Ok((
                OsStr::from_bytes(name).to_os_string(),
                entry.file_type() == FileType::Directory,
            )))
        }))
    }

    pub(crate) fn rename_exclusive(src: &Path, dst: &Path) -> AppResult<()> {
        rename(src, dst, None)
    }

    pub(crate) fn rename_checked(src: &Path, dst: &Path, expected: &FileIdentity) -> AppResult<()> {
        rename(src, dst, Some(expected))
    }

    fn rename(src: &Path, dst: &Path, expected: Option<&FileIdentity>) -> AppResult<()> {
        let (from, old) = parent(src, false)?;
        let (to, new) = parent(dst, expected.is_none())?;
        if let Some(expected) = expected {
            if identity_at(&from, old)?.as_ref() != Some(expected) {
                return Err(wrong_identity(src));
            }
        } else {
            let source = fs::statat(&from, old, AtFlags::SYMLINK_NOFOLLOW).map_err(io)?;
            if !matches!(
                FileType::from_raw_mode(source.st_mode),
                FileType::RegularFile | FileType::Directory
            ) {
                return Err(AppError::Denied("不支持移动链接或特殊文件".into()));
            }
        }
        match fs::renameat_with(&from, old, &to, new, RenameFlags::NOREPLACE) {
            Err(Errno::EXIST) => Err(AppError::Conflict(format!("目标已存在: {}", dst.display()))),
            result => result.map_err(io),
        }
    }

    pub(crate) fn identity(path: &Path) -> AppResult<Option<FileIdentity>> {
        match parent(path, false) {
            Ok((dir, name)) => identity_at(&dir, name),
            Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn identity_at(dir: &OwnedFd, name: &OsStr) -> AppResult<Option<FileIdentity>> {
        fn identified(kind: FileType, single_link: bool) -> bool {
            kind == FileType::Directory || (kind == FileType::RegularFile && single_link)
        }
        #[cfg(target_os = "linux")]
        {
            use fs::StatxFlags;
            let required = StatxFlags::TYPE | StatxFlags::INO | StatxFlags::NLINK;
            match fs::statx(
                dir,
                name,
                AtFlags::SYMLINK_NOFOLLOW,
                required | StatxFlags::BTIME,
            ) {
                Ok(s) if StatxFlags::from_bits_truncate(s.stx_mask).contains(required) => {
                    return Ok(identified(
                        FileType::from_raw_mode(s.stx_mode.into()),
                        s.stx_nlink == 1,
                    )
                    .then(|| FileIdentity {
                        volume: u64::from(s.stx_dev_major) << 32 | u64::from(s.stx_dev_minor),
                        object: u128::from(s.stx_ino).to_le_bytes(),
                        birth: StatxFlags::from_bits_truncate(s.stx_mask)
                            .contains(StatxFlags::BTIME)
                            .then_some((s.stx_btime.tv_sec, s.stx_btime.tv_nsec)),
                    }));
                }
                Err(Errno::NOENT) => return Ok(None),
                Ok(_) | Err(Errno::NOSYS | Errno::INVAL | Errno::OPNOTSUPP | Errno::PERM) => {}
                Err(error) => return Err(io(error)),
            }
        }
        let s = match fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(s) => s,
            Err(Errno::NOENT) => return Ok(None),
            Err(error) => return Err(io(error)),
        };
        Ok(
            identified(FileType::from_raw_mode(s.st_mode), s.st_nlink == 1).then(|| FileIdentity {
                volume: u64::from(fs::major(s.st_dev)) << 32 | u64::from(fs::minor(s.st_dev)),
                object: u128::from(s.st_ino).to_le_bytes(),
                #[cfg(target_os = "macos")]
                birth: Some((s.st_birthtime, s.st_birthtime_nsec as u32)),
                #[cfg(target_os = "linux")]
                birth: None,
            }),
        )
    }

    pub(crate) struct StagingDirectory {
        path: PathBuf,
        root: OwnedFd,
    }

    impl StagingDirectory {
        pub(crate) fn create(path: &Path) -> AppResult<Self> {
            let (parent, name) = parent(path, false)?;
            fs::mkdirat(&parent, name, Mode::from_raw_mode(0o700)).map_err(io)?;
            let root = fs::openat(&parent, name, DIRECTORY, Mode::empty()).map_err(io)?;
            Ok(Self {
                path: path.into(),
                root,
            })
        }

        pub(crate) fn path(&self) -> &Path {
            &self.path
        }

        fn create_subdirectory(&self, relative: &Path) -> AppResult<OwnedFd> {
            let components = relative_components(relative)?;
            descend(self.root.try_clone()?, components, true)
        }

        pub(crate) fn create_dir_all(&self, relative: &Path) -> AppResult<()> {
            self.create_subdirectory(relative)?;
            Ok(())
        }

        pub(crate) fn create_file(&self, relative: &Path) -> AppResult<File> {
            let _ = relative_components(relative)?;
            let (base, name) = relative
                .parent()
                .zip(relative.file_name())
                .ok_or_else(|| invalid_path(relative))?;
            create_new_at(&self.create_subdirectory(base)?, name)
        }

        // Caller-visible paths must still address the new directory, not a
        // replacement tree that appeared while its descriptor was in use.
        pub(crate) fn verify_path(&self) -> AppResult<()> {
            let original = fs::fstat(&self.root).map_err(io)?;
            let current = fs::fstat(directory(&self.path, false)?).map_err(io)?;
            if current.st_dev != original.st_dev || current.st_ino != original.st_ino {
                return Err(AppError::Conflict("解压暂存目录已被替换".into()));
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
#[path = "safe_fs_windows.rs"]
mod platform;

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::{relative_components, wrong_identity, FileIdentity};
    use crate::error::{AppError, AppResult};
    use std::{
        fs::{self, File, OpenOptions},
        path::{Path, PathBuf},
    };

    pub(crate) fn directory_entries(
        path: &Path,
        expected: &FileIdentity,
    ) -> AppResult<impl Iterator<Item = AppResult<(std::ffi::OsString, bool)>>> {
        if identity(path)?.as_ref() != Some(expected) {
            return Err(wrong_identity(path));
        }
        let entries = fs::read_dir(path)?;
        if identity(path)?.as_ref() != Some(expected) {
            return Err(wrong_identity(path));
        }
        Ok(entries.map(|entry| {
            let entry = entry?;
            Ok((entry.file_name(), entry.file_type()?.is_dir()))
        }))
    }
    pub(crate) fn create_dir_all(path: &Path) -> AppResult<()> {
        Ok(fs::create_dir_all(path)?)
    }

    fn create_parents(path: &Path) -> AppResult<()> {
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        Ok(())
    }

    pub(crate) fn create_dir(path: &Path) -> AppResult<()> {
        create_parents(path)?;
        Ok(fs::create_dir(path)?)
    }

    pub(crate) fn create_new(path: &Path) -> AppResult<File> {
        create_parents(path)?;
        Ok(OpenOptions::new().write(true).create_new(true).open(path)?)
    }

    pub(crate) fn open_regular(path: &Path) -> AppResult<File> {
        if !fs::symlink_metadata(path)?.is_file() {
            return Err(AppError::Denied("只允许打开普通文件".into()));
        }
        let file = File::open(path)?;
        if !file.metadata()?.is_file() {
            return Err(AppError::Denied("只允许打开普通文件".into()));
        }
        Ok(file)
    }

    pub(crate) fn verify_regular_at(_file: &File, _path: &Path) -> AppResult<()> {
        Err(AppError::Unsupported(
            "当前平台不支持验证普通文件对象".into(),
        ))
    }

    pub(crate) fn rename_checked(
        src: &Path,
        _dst: &Path,
        _expected: &FileIdentity,
    ) -> AppResult<()> {
        Err(wrong_identity(src))
    }

    pub(crate) fn identity(path: &Path) -> AppResult<Option<FileIdentity>> {
        Err(wrong_identity(path))
    }

    pub(crate) fn rename_exclusive(src: &Path, dst: &Path) -> AppResult<()> {
        match fs::symlink_metadata(dst) {
            Ok(_) => return Err(AppError::Conflict(format!("目标已存在: {}", dst.display()))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        create_parents(dst)?;
        Ok(fs::rename(src, dst)?)
    }

    fn require_directory(path: &Path) -> AppResult<()> {
        let meta = fs::symlink_metadata(path)?;
        if !meta.file_type().is_dir() {
            return Err(AppError::Denied("暂存路径包含链接或非目录".into()));
        }
        Ok(())
    }

    pub(crate) struct StagingDirectory {
        path: PathBuf,
    }

    impl StagingDirectory {
        pub(crate) fn create(path: &Path) -> AppResult<Self> {
            let base = path
                .parent()
                .ok_or_else(|| AppError::Denied("无效暂存路径".into()))?;
            require_directory(base)?;
            if !base.is_absolute() || base.canonicalize()? != base {
                return Err(AppError::Denied("暂存父目录不是无链接的绝对路径".into()));
            }
            fs::create_dir(path)?;
            let staging = Self { path: path.into() };
            staging.verify_path()?;
            Ok(staging)
        }

        pub(crate) fn path(&self) -> &Path {
            &self.path
        }

        pub(crate) fn create_dir_all(&self, relative: &Path) -> AppResult<()> {
            let components = relative_components(relative)?;
            self.verify_path()?;
            let mut path = self.path.clone();
            for component in components {
                path.push(component.as_os_str());
                match fs::create_dir(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
                require_directory(&path)?;
            }
            Ok(())
        }

        pub(crate) fn create_file(&self, relative: &Path) -> AppResult<File> {
            let _ = relative_components(relative)?;
            let base = relative
                .parent()
                .ok_or_else(|| AppError::Denied("无效暂存条目路径".into()))?;
            self.create_dir_all(base)?;
            Ok(OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.path.join(relative))?)
        }

        pub(crate) fn verify_path(&self) -> AppResult<()> {
            require_directory(&self.path)?;
            if self.path.canonicalize()? != self.path {
                return Err(AppError::Conflict("解压暂存目录已被替换".into()));
            }
            Ok(())
        }
    }
}

pub(crate) use platform::{
    create_dir, create_dir_all, create_new, directory_entries, identity, open_regular,
    rename_checked, rename_exclusive, verify_regular_at, StagingDirectory,
};

#[cfg(all(test, windows))]
#[path = "safe_fs_windows_tests.rs"]
mod windows_tests;

#[cfg(all(test, any(target_os = "linux", target_os = "macos", windows)))]
mod tests {
    use super::{directory_entries, identity, open_regular, verify_regular_at};
    use std::{
        fs::{self, File},
        path::PathBuf,
    };

    fn regular_fixture() -> (tempfile::TempDir, PathBuf, File) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("mod.bin");
        fs::write(&path, b"installed").unwrap();
        let file = open_regular(&path).unwrap();
        (dir, path, file)
    }

    #[test]
    fn verify_regular_at_accepts_unchanged_file() {
        let (_dir, path, file) = regular_fixture();
        verify_regular_at(&file, &path).unwrap();
    }

    #[test]
    fn directory_entries_rejects_a_replaced_directory() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mods = root.join("mods");
        fs::create_dir(&mods).unwrap();
        fs::write(mods.join("test.jar"), b"fixture").unwrap();
        let original = identity(&mods).unwrap().unwrap();
        assert!(directory_entries(&mods, &original)
            .unwrap()
            .any(|entry| { entry.is_ok_and(|(name, is_dir)| name == "test.jar" && !is_dir) }));
        fs::rename(&mods, root.join("old-mods")).unwrap();
        fs::create_dir(&mods).unwrap();
        assert!(directory_entries(&mods, &original).is_err());
    }

    #[test]
    fn verify_regular_at_rejects_replaced_file_with_same_contents() {
        let (_dir, path, file) = regular_fixture();
        fs::rename(&path, path.with_extension("old")).unwrap();
        fs::write(&path, b"installed").unwrap();
        assert!(verify_regular_at(&file, &path).is_err());
    }

    #[test]
    fn verify_regular_at_rejects_new_hard_link() {
        let (_dir, path, file) = regular_fixture();
        fs::hard_link(&path, path.with_extension("link")).unwrap();
        assert!(verify_regular_at(&file, &path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn verify_regular_at_does_not_follow_replaced_leaf() {
        let (_dir, path, file) = regular_fixture();
        let moved = path.with_extension("old");
        fs::rename(&path, &moved).unwrap();
        std::os::unix::fs::symlink(moved, &path).unwrap();
        assert!(verify_regular_at(&file, &path).is_err());
    }
}
