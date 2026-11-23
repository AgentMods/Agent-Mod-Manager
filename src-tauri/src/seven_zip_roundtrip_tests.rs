use crate::archive::{compress_snapshot, extract_archive, list_archive};
use std::{fs, path::PathBuf};

#[cfg(unix)]
use std::path::Path;

#[cfg(unix)]
fn create_real_7z_archive(parent: &Path, source_dir: &Path, archive_name: &str) -> PathBuf {
    let archive = parent.join(archive_name);
    let source_name = source_dir
        .file_name()
        .expect("source directory name is present");
    let mut command = crate::seven_zip::command().expect("resolve configured 7-Zip executable");
    let output = command
        .current_dir(parent)
        .args(["a", "-t7z", "-mx=1", "-bd", "-y", "-r", "--"])
        .arg(archive.file_name().expect("archive name is present"))
        .arg(Path::new(".").join(source_name))
        .output()
        .expect("start the configured 7-Zip executable");
    assert!(
        output.status.success(),
        "7-Zip folder compression failed ({}):\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        archive.is_file(),
        "7-Zip did not create {}",
        archive.display()
    );
    archive
}

#[test]
fn compress_snapshot_roundtrips_unicode_spaces_empty_and_switch_like_names() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("@来源 -目录");
    let empty_directory = source.join("空 子目录");
    let nested_directory = source.join("-子 路径").join("二级 中文");
    fs::create_dir_all(&nested_directory).unwrap();
    fs::create_dir(&empty_directory).unwrap();

    let empty_file = PathBuf::from("-空 文件.bin");
    let nested_file = PathBuf::from("-子 路径")
        .join("二级 中文")
        .join("@数据 文件.txt");
    let contents = "真实 7-Zip 往返内容\n".as_bytes();
    fs::write(source.join(&empty_file), b"").unwrap();
    fs::write(source.join(&nested_file), contents).unwrap();

    for format in ["zip", "7z"] {
        let archive = root.join(format!("归档 {format}"));
        compress_snapshot(&source, &archive, format).unwrap();
        let listing = list_archive(&archive).unwrap();
        assert_eq!(listing.format, format);
        assert_eq!(listing.stripped_root.as_deref(), Some("@来源 -目录"));
        assert_eq!(listing.total_size, contents.len() as u64);

        let member = |path: &str| {
            listing
                .entries
                .iter()
                .find(|entry| entry.path.replace('\\', "/").trim_end_matches('/') == path)
        };
        let empty_member =
            member("@来源 -目录/-空 文件.bin").expect("archive listing contains the empty file");
        assert!(!empty_member.is_dir);
        assert_eq!(empty_member.size, 0);
        let nested_member = member("@来源 -目录/-子 路径/二级 中文/@数据 文件.txt")
            .expect("archive listing contains the nested Unicode file");
        assert!(!nested_member.is_dir);
        assert_eq!(nested_member.size, contents.len() as u64);
        assert!(
            listing.entries.iter().any(|entry| {
                entry.is_dir
                    && entry.path.replace('\\', "/").trim_end_matches('/')
                        == "@来源 -目录/空 子目录"
            }),
            "archive listing preserves the actual empty source directory"
        );

        let destination = root.join(format!("解压 {format}"));
        let written =
            extract_archive(&archive, &destination, listing.stripped_root.as_deref()).unwrap();
        assert_eq!(written.len(), 2);
        assert!(written.contains(&destination.join(&empty_file)));
        assert!(written.contains(&destination.join(&nested_file)));
        assert!(fs::read(destination.join(&empty_file)).unwrap().is_empty());
        assert_eq!(fs::read(destination.join(&nested_file)).unwrap(), contents);
        let extracted_empty_directory = destination.join("空 子目录");
        assert!(extracted_empty_directory.is_dir());
        assert!(fs::read_dir(extracted_empty_directory)
            .unwrap()
            .next()
            .is_none());
    }
}

#[test]
fn compress_snapshot_roundtrips_an_empty_source_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("空源目录");
    fs::create_dir(&source).unwrap();

    for format in ["zip", "7z"] {
        let archive = root.join(format!("空目录归档 {format}"));
        compress_snapshot(&source, &archive, format).unwrap();
        let listing = list_archive(&archive).unwrap();
        assert_eq!(listing.format, format);
        assert_eq!(listing.total_size, 0);
        assert_eq!(listing.stripped_root.as_deref(), Some("空源目录"));
        assert!(listing.entries.iter().any(|entry| {
            entry.is_dir && entry.path.trim_end_matches(['/', '\\']) == "空源目录"
        }));

        let destination = root.join(format!("空目录解压 {format}"));
        extract_archive(&archive, &destination, listing.stripped_root.as_deref()).unwrap();
        assert!(destination.is_dir());
        assert!(fs::read_dir(destination).unwrap().next().is_none());
    }
}

#[test]
fn compress_snapshot_never_overwrites_existing_outputs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("data.txt"), b"payload").unwrap();

    // Exercise the successful 7-Zip invocation and atomic publish, then prove
    // a second invocation cannot replace that real archive.
    let real_archive = root.join("real-no-clobber");
    compress_snapshot(&source, &real_archive, "7z").unwrap();
    let original_archive = fs::read(&real_archive).unwrap();
    assert!(compress_snapshot(&source, &real_archive, "7z").is_err());
    assert_eq!(fs::read(&real_archive).unwrap(), original_archive);

    let existing_file = root.join("existing-output");
    fs::write(&existing_file, b"keep me").unwrap();
    assert!(compress_snapshot(&source, &existing_file, "7z").is_err());
    assert_eq!(fs::read(&existing_file).unwrap(), b"keep me");

    let existing_directory = root.join("existing-directory");
    fs::create_dir(&existing_directory).unwrap();
    fs::write(existing_directory.join("keep.txt"), b"keep me too").unwrap();
    assert!(compress_snapshot(&source, &existing_directory, "zip").is_err());
    assert_eq!(
        fs::read(existing_directory.join("keep.txt")).unwrap(),
        b"keep me too"
    );

    let invalid_format_output = root.join("invalid-format-output");
    assert!(compress_snapshot(&source, &invalid_format_output, "tar").is_err());
    assert!(!invalid_format_output.exists());
}

#[cfg(unix)]
#[test]
fn real_7zip_archive_with_ascii_colon_entry_is_rejected_before_writes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("来源");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("危险:数据.txt"), b"must not be extracted").unwrap();
    let archive = create_real_7z_archive(&root, &source, "含非法成员.7z");

    assert!(list_archive(&archive).is_err());
    let destination = root.join("不应创建");
    assert!(extract_archive(&archive, &destination, None).is_err());
    assert!(!destination.exists());
}
