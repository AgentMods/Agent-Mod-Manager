use crate::{
    error::{AppError, AppResult},
    safe_fs,
    transaction::FileEngine,
    util,
};
use serde::Serialize;
use std::{io::Read, path::Path};

#[derive(Debug, Serialize)]
pub struct TextLine {
    pub line: usize,
    pub text: String,
}
#[derive(Debug, Serialize)]
pub struct TextFile {
    pub lines: Vec<TextLine>,
    pub truncated: bool,
}

pub fn read_dir(engine: &FileEngine, path: &Path) -> AppResult<Vec<util::DirEntryInfo>> {
    let path = engine.paths.guard(path, &engine.roots()?)?;
    util::list_dir_shallow(&path, 2000)
}
pub fn read_file(engine: &FileEngine, path: &Path) -> AppResult<TextFile> {
    let path = engine.paths.guard(path, &engine.roots()?)?;
    let mut bytes = Vec::new();
    safe_fs::open_regular(&path)?
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > 256 * 1024;
    bytes.truncate(256 * 1024);
    if bytes.contains(&0) {
        return Err(AppError::Invalid(
            "此文件是二进制内容，不能作为文本读取".into(),
        ));
    }
    let content = String::from_utf8_lossy(&bytes);
    let line_truncated = content.lines().count() > 2000;
    Ok(TextFile {
        lines: content
            .lines()
            .take(2000)
            .enumerate()
            .map(|(i, text)| TextLine {
                line: i + 1,
                text: text.into(),
            })
            .collect(),
        truncated: truncated || line_truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_numbered_text_and_registered_directory_reads() {
        let (_temp, engine, game) = crate::transaction::tests::setup();
        let file = game.join("log.txt");
        std::fs::write(&file, "first\nsecond\n").unwrap();
        let text = read_file(&engine, &file).unwrap();
        assert_eq!(text.lines[1].line, 2);
        assert_eq!(text.lines[1].text, "second");
        assert!(!text.truncated);
        assert_eq!(read_dir(&engine, &game).unwrap().len(), 1);
        std::fs::write(&file, "line\n".repeat(3000)).unwrap();
        let text = read_file(&engine, &file).unwrap();
        assert_eq!(text.lines.len(), 2000);
        assert!(text.truncated);
    }
    #[test]
    fn binary_and_outside_files_are_not_returned() {
        let (temp, engine, game) = crate::transaction::tests::setup();
        let binary = game.join("binary.dat");
        std::fs::write(&binary, b"a\0b").unwrap();
        assert!(read_file(&engine, &binary).is_err());
        let outside = temp.path().join("outside.txt");
        std::fs::write(&outside, b"private").unwrap();
        assert!(read_file(&engine, &outside).is_err());
        assert!(read_dir(&engine, temp.path()).is_err());
    }

    #[test]
    fn credentials_and_env_are_not_readable_by_file_tools() {
        let (_temp, engine, game) = crate::transaction::tests::setup();
        let private = engine.paths.root.join("private");
        std::fs::create_dir(&private).unwrap();
        for path in [
            private.join(".env"),
            private.join("credentials.json"),
            game.join(".env"),
        ] {
            std::fs::write(&path, "private-material").unwrap();
            assert!(read_file(&engine, &path).is_err());
            assert!(engine
                .plan_file_ops(
                    vec![crate::transaction::FileOp::Copy {
                        src: path,
                        dst: game.join("leak.txt")
                    }],
                    "must reject".into()
                )
                .is_err());
        }
        assert!(read_dir(&engine, &private).is_err());
    }
}
