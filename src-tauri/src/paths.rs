use crate::error::{AppError, AppResult};
use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

/// Spelling for a local path shown or returned to a user. Keep canonical paths for
/// filesystem guards, database identities and transaction journals.
pub(crate) fn user_path(path: &Path) -> Cow<'_, Path> {
    #[cfg(windows)]
    if let Some(short) = path.to_str().and_then(ordinary_windows_path) {
        return short;
    }
    Cow::Borrowed(path)
}

pub(crate) fn display_path(path: &Path) -> String {
    user_path(path).display().to_string()
}

pub(crate) fn serialize_user_path<S: serde::Serializer, P: AsRef<Path> + ?Sized>(
    path: &P,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(user_path(path.as_ref()).as_ref(), serializer)
}

pub(crate) fn set_user_path_text(text: &mut String) {
    let path = Path::new(text);
    let shown = user_path(path);
    if shown.as_os_str() != path.as_os_str() {
        *text = shown.to_string_lossy().into_owned();
    }
}

#[cfg(any(windows, test))]
fn ordinary_windows_path(text: &str) -> Option<Cow<'_, Path>> {
    let body = text.strip_prefix(r"\\?\")?;
    let bytes = body.as_bytes();
    let unc = body
        .get(..4)
        .is_some_and(|s| s.eq_ignore_ascii_case(r"UNC\"));
    let components = if unc {
        let tail = &body[4..];
        let (server, share_and_rest) = tail.split_once('\\')?;
        if server.is_empty() || share_and_rest.is_empty() || share_and_rest.starts_with('\\') {
            return None;
        }
        tail
    } else if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && &bytes[1..3] == br":\" {
        &body[3..]
    } else {
        return None;
    };
    // Dots and trailing spaces can be literal names in the verbatim namespace.
    // Only remove the prefix when a path copied from the UI names the same object.
    if !components.is_empty()
        && !components
            .strip_suffix('\\')
            .unwrap_or(components)
            .split('\\')
            .all(|name| valid_windows_component(name) && !windows_device_name(name))
    {
        return None;
    }
    Some(if unc {
        Cow::Owned(PathBuf::from(format!(r"\\{components}")))
    } else {
        Cow::Borrowed(Path::new(body))
    })
}

/// Normalize a path lexically: resolve `.` and `..` without touching the filesystem.
pub fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve existing components before handling `..`, allowing a missing tail.
/// A dangling symlink is an existing component, never a missing tail.
pub fn canonicalize_lenient(p: &Path) -> AppResult<PathBuf> {
    let absolute = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()?.join(p)
    };
    // In particular, `link/..` means the parent of the link's *target*.
    // Lexically normalizing first would silently change that meaning.
    match absolute.canonicalize() {
        Ok(c) => return Ok(c),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut resolved = PathBuf::new();
    let mut components = absolute.components().peekable();
    while let Some(component) = components.next() {
        match component {
            Component::Prefix(_) | Component::RootDir => resolved.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                match std::fs::symlink_metadata(&resolved) {
                    Ok(meta) if !meta.is_dir() => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::NotADirectory,
                            format!("{} 不是目录", display_path(&resolved)),
                        )
                        .into());
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
                resolved.pop(); // At the filesystem root, pop leaves it unchanged.
            }
            Component::Normal(name) => {
                resolved.push(name);
                match std::fs::symlink_metadata(&resolved) {
                    // canonicalize must succeed for an existing link, including
                    // links whose targets have missing components.
                    Ok(_) => {
                        resolved = resolved.canonicalize()?;
                        // Windows may report a child of a regular file as NotFound,
                        // which would otherwise be mistaken for a creatable tail.
                        if components.peek().is_some() && !std::fs::metadata(&resolved)?.is_dir() {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::NotADirectory,
                                format!("{} 不是目录", display_path(&resolved)),
                            )
                            .into());
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }
    Ok(resolved)
}

/// True when `child` is inside `root` (or equal to it). Both are canonicalized.
pub fn is_within(child: &Path, root: &Path) -> bool {
    child.starts_with(root)
}

/// Compare canonical directory paths, including absent destinations under an existing parent.
pub(crate) fn contains_path(parent: &Path, child: &Path) -> bool {
    if !cfg!(windows) {
        return child.starts_with(parent);
    }
    let mut parts = child.components();
    parent.components().all(|expected| {
        parts.next().is_some_and(|actual| {
            actual
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy())
        })
    })
}

pub(crate) fn overlaps(left: &Path, right: &Path) -> bool {
    contains_path(right, left) || contains_path(left, right)
}

/// Validate that a target path stays inside one of the allowed roots.
pub fn ensure_within(target: &Path, roots: &[PathBuf]) -> AppResult<PathBuf> {
    let canon = canonicalize_lenient(target)?;
    for root in roots {
        let root_canon = canonicalize_lenient(root)?;
        if is_within(&canon, &root_canon) {
            return Ok(canon);
        }
    }
    let shown: Vec<String> = roots.iter().map(|r| display_path(r)).collect();
    Err(AppError::PathEscape(format!(
        "{} 不在允许的目录内 ({})",
        display_path(&canon),
        shown.join(", ")
    )))
}

fn valid_windows_component(name: &str) -> bool {
    !name.is_empty()
        && !name
            .chars()
            .any(|c| c.is_control() || "/<>:\"|?*".contains(c))
        && !name.ends_with(['.', ' '])
}

fn windows_device_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end_matches(' ')
        .to_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|n| {
            matches!(
                n,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    })
}

/// Reject any archive entry whose relative path tries to escape the destination.
pub fn safe_join(base: &Path, entry: &str) -> AppResult<PathBuf> {
    if entry.is_empty() {
        return Err(AppError::UnsafeArchive("空的归档条目名".into()));
    }
    let cleaned = entry.replace('\\', "/");
    if cleaned.starts_with('/') || cleaned.starts_with('~') {
        return Err(AppError::UnsafeArchive(format!("绝对路径条目: {entry}")));
    }
    if cleaned.len() > 1 && cleaned.as_bytes()[1] == b':' {
        return Err(AppError::UnsafeArchive(format!("盘符路径条目: {entry}")));
    }
    let mut rel = PathBuf::new();
    for part in cleaned.split('/') {
        match part {
            "" | "." => continue,
            ".." => return Err(AppError::UnsafeArchive(format!("路径穿越条目: {entry}"))),
            p => {
                // Apply Windows rules on every host so a preview on Unix cannot
                // approve an archive that becomes a device or ADS on Windows.
                if !valid_windows_component(p) {
                    return Err(AppError::UnsafeArchive(format!("不安全的条目名: {entry}")));
                }
                if windows_device_name(p) {
                    return Err(AppError::UnsafeArchive(format!("设备名称条目: {entry}")));
                }
                rel.push(p);
            }
        }
    }
    if rel.as_os_str().is_empty() {
        return Err(AppError::UnsafeArchive(format!("无效条目: {entry}")));
    }
    Ok(base.join(rel))
}

/// Strip a single common top-level directory (the classic `ModName/` wrapper).
pub fn strip_single_root(entries: &[String]) -> Option<String> {
    let mut root: Option<String> = None;
    for e in entries {
        let cleaned = e.replace('\\', "/");
        let mut parts = cleaned.split('/').filter(|p| !p.is_empty() && *p != ".");
        let Some(first) = parts.next() else { continue };
        if first == "__MACOSX" {
            continue;
        }
        if parts.next().is_none() && !cleaned.ends_with('/') {
            // A bare file at the archive root: nothing to strip.
            return None;
        }
        match &root {
            None => root = Some(first.to_string()),
            Some(r) if r == first => {}
            Some(_) => return None,
        }
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_user_paths_only_shorten_equivalent_drive_and_share_names() {
        for (raw, shown) in [
            (r"\\?\E:\Games\我的游戏", r"E:\Games\我的游戏"),
            (r"\\?\C:\", r"C:\"),
            (r"\\?\UNC\nas\Mods", r"\\nas\Mods"),
            (r"\\?\UNC\Nas\Share\Games\", r"\\Nas\Share\Games\"),
        ] {
            assert_eq!(ordinary_windows_path(raw).unwrap().as_os_str(), shown);
            assert!(ordinary_windows_path(shown).is_none());
        }
        for raw in [
            r"\\?\Volume{abc}\folder",
            r"\\.\PIPE\service",
            r"\\?\UNC\host",
            r"\\?\UNC\.\share",
            r"\\?\UNC\host\share\NUL.txt",
            r"\\?\C:\Games\a.",
            r"\\?\C:\Games\a ",
            r"\\?\C:\Games\..\b",
            r"\\?\C:\Games\.\b",
            r"\\?\C:\Games\a/b",
            r"\\?\C:\COM¹.log",
        ] {
            assert!(ordinary_windows_path(raw).is_none(), "{raw}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_user_paths_preserve_literal_backslashes_and_case() {
        for raw in [r"/Games/Mods\Data", r"/games/mods\data", r"\\?\C:\Games"] {
            assert_eq!(user_path(Path::new(raw)).as_os_str(), raw);
        }
    }

    #[test]
    fn normalize_removes_dotdot() {
        assert_eq!(
            lexical_normalize(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
    }

    #[test]
    fn safe_join_blocks_traversal() {
        let base = PathBuf::from("/tmp/x");
        assert!(safe_join(&base, "../etc/passwd").is_err());
        assert!(safe_join(&base, "/etc/passwd").is_err());
        assert!(safe_join(&base, "C:/windows/system32").is_err());
        assert_eq!(
            safe_join(&base, "mod/data/file.txt").unwrap(),
            PathBuf::from("/tmp/x/mod/data/file.txt")
        );
    }

    #[test]
    fn strip_detects_single_root() {
        let entries = vec!["ModA/file.txt".to_string(), "ModA/sub/x.dll".to_string()];
        assert_eq!(strip_single_root(&entries).as_deref(), Some("ModA"));
        let multi = vec!["a/x".to_string(), "b/y".to_string()];
        assert!(strip_single_root(&multi).is_none());
    }

    #[test]
    fn within_works() {
        assert!(is_within(Path::new("/a/b/c"), Path::new("/a/b")));
        assert!(!is_within(Path::new("/a/bc"), Path::new("/a/b")));
    }

    #[test]
    fn normalize_preserves_relative_parents_and_filesystem_root() {
        assert_eq!(
            lexical_normalize(Path::new("../../a/../b")),
            PathBuf::from("../../b")
        );
        assert_eq!(
            lexical_normalize(Path::new("/../../a")),
            PathBuf::from("/a")
        );
    }

    #[test]
    fn safe_join_rejects_portable_special_names_and_ads() {
        for name in [
            "NUL",
            "dir/con.txt",
            "COM1.log",
            "Lpt9",
            "COM¹.txt",
            "LPT²",
            "CONIN$",
            "AUX .txt",
            "file:stream",
            "dir/file::$DATA",
            "trailing.",
            "trailing ",
            "dir/.. /x",
            "dir/a\0b",
            "a\nb",
            "a?b",
            "a*b",
            "a|b",
            "a<b",
            "a>b",
            "a\"b",
            "\\\\server\\share",
            "\\?\\C:\\file",
        ] {
            assert!(safe_join(Path::new("out"), name).is_err(), "{name:?}");
        }
        assert!(safe_join(Path::new("out"), "COM10/safe.txt").is_ok());
        assert!(safe_join(Path::new("out"), "mod\\data\\safe.txt").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn canonicalize_resolves_symlink_before_parent_for_existing_and_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let allowed = dir.path().join("allowed");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(outside.join("nested")).unwrap();
        std::fs::create_dir(&allowed).unwrap();
        std::fs::write(outside.join("existing"), b"x").unwrap();
        std::os::unix::fs::symlink(outside.join("nested"), allowed.join("link")).unwrap();
        for tail in ["existing", "new/deeper"] {
            let path = allowed.join("link/..").join(tail);
            assert_eq!(
                canonicalize_lenient(&path).unwrap(),
                outside.canonicalize().unwrap().join(tail)
            );
            assert!(ensure_within(&path, std::slice::from_ref(&allowed)).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_not_a_missing_tail_even_before_parent() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(dir.path().join("missing"), dir.path().join("dangling"))
            .unwrap();
        for tail in [
            "dangling",
            "dangling/new",
            "dangling/../new",
            "missing/../dangling/new",
        ] {
            assert!(
                canonicalize_lenient(&dir.path().join(tail)).is_err(),
                "{tail}"
            );
        }
        assert_eq!(
            canonicalize_lenient(&dir.path().join("missing/../safe/new")).unwrap(),
            dir.path().canonicalize().unwrap().join("safe/new")
        );
    }

    #[test]
    fn existing_regular_file_is_not_an_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file"), b"x").unwrap();
        assert!(canonicalize_lenient(&dir.path().join("file/../new")).is_err());
        assert!(canonicalize_lenient(&dir.path().join("file/new")).is_err());
    }
}
