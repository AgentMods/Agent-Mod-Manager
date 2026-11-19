use super::{extract_archive, extract_verified_archive, list_archive};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::symlink;

fn canonical_tempdir() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    (temp, root)
}

fn write_archive(path: &Path) -> String {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default());
    zip.add_directory("Wrapper/", options).unwrap();
    zip.start_file("Wrapper/data/hello.txt", options).unwrap();
    zip.write_all(b"hello").unwrap();
    zip.start_file("Wrapper/empty.txt", options).unwrap();
    drop(zip.finish().unwrap());
    hex::encode(Sha256::digest(fs::read(path).unwrap()))
}

fn assert_absent(path: &Path) {
    assert!(
        matches!(fs::symlink_metadata(path), Err(err) if err.kind() == ErrorKind::NotFound),
        "expected no filesystem entry at {}",
        path.display()
    );
}

#[test]
fn verified_extraction_rejects_wrong_digest_without_creating_destination() {
    let (_temp, root) = canonical_tempdir();
    let archive = root.join("mod.zip");
    let actual = write_archive(&archive);
    let dest = root.join("staged");
    let wrong = "0".repeat(64);
    assert_ne!(wrong, actual);

    assert!(extract_verified_archive(&archive, &dest, &wrong).is_err());
    assert_absent(&dest);
}

#[test]
fn verified_extraction_reports_exact_listing_and_hashes_of_written_files() {
    let (_temp, root) = canonical_tempdir();
    let archive = root.join("mod.zip");
    let digest = write_archive(&archive);
    let dest = root.join("verified");

    let verified = extract_verified_archive(&archive, &dest, &digest).unwrap();
    let listing = list_archive(&archive).unwrap();
    assert_eq!(
        serde_json::to_value(&verified.listing).unwrap(),
        serde_json::to_value(&listing).unwrap()
    );
    assert_eq!(listing.format, "zip");
    assert_eq!(listing.total_size, 5);
    assert_eq!(listing.stripped_root.as_deref(), Some("Wrapper"));
    assert_eq!(
        listing
            .entries
            .iter()
            .map(|e| (e.path.as_str(), e.is_dir, e.size, e.compressed_size))
            .collect::<Vec<_>>(),
        [
            ("Wrapper/", true, 0, 0),
            ("Wrapper/data/hello.txt", false, 5, 5),
            ("Wrapper/empty.txt", false, 0, 0),
        ]
    );

    let hello = dest.join("Wrapper").join("data").join("hello.txt");
    let empty = dest.join("Wrapper").join("empty.txt");
    assert_eq!(fs::read(&hello).unwrap(), b"hello");
    assert!(fs::read(&empty).unwrap().is_empty());
    assert_eq!(
        verified.files,
        BTreeMap::from([
            (
                hello,
                "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".to_owned(),
            ),
            (
                empty,
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_owned(),
            ),
        ])
    );

    let stripped = root.join("stripped");
    assert_eq!(
        extract_archive(&archive, &stripped, listing.stripped_root.as_deref()).unwrap(),
        [
            stripped.join("data").join("hello.txt"),
            stripped.join("empty.txt")
        ]
    );
}

#[cfg(unix)]
#[test]
fn extraction_rejects_symlinked_staging_parent_without_writing() {
    let (_temp, root) = canonical_tempdir();
    let archive = root.join("mod.zip");
    let digest = write_archive(&archive);
    let outside = root.join("elsewhere");
    let parent = root.join("staging");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, &parent).unwrap();

    assert!(extract_archive(&archive, &parent.join("legacy"), None).is_err());
    assert!(extract_verified_archive(&archive, &parent.join("verified"), &digest).is_err());
    assert_eq!(fs::read_link(&parent).unwrap(), outside);
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
}

#[cfg(unix)]
#[test]
fn archive_wrappers_reject_source_leaf_symlinks_even_with_a_matching_digest() {
    let (_temp, root) = canonical_tempdir();
    let archive = root.join("mod.zip");
    let digest = write_archive(&archive);
    let legacy_dest = root.join("legacy");
    let verified_dest = root.join("verified");

    for name in ["linked.zip", "linked"] {
        let linked_archive = root.join(name);
        symlink(&archive, &linked_archive).unwrap();
        assert!(list_archive(&linked_archive).is_err(), "{name}");
        assert!(
            extract_archive(&linked_archive, &legacy_dest, None).is_err(),
            "{name}"
        );
        assert!(
            extract_verified_archive(&linked_archive, &verified_dest, &digest).is_err(),
            "{name}"
        );
        assert_absent(&legacy_dest);
        assert_absent(&verified_dest);
    }
}

#[test]
fn verified_extraction_preserves_existing_staging_files_and_directories() {
    let (_temp, root) = canonical_tempdir();
    let archive = root.join("mod.zip");
    let digest = write_archive(&archive);
    let file = root.join("staged-file");
    let dir = root.join("staged-dir");
    let kept = dir.join("keep.txt");
    fs::write(&file, b"existing file").unwrap();
    fs::create_dir(&dir).unwrap();
    fs::write(&kept, b"existing directory contents").unwrap();

    assert!(extract_verified_archive(&archive, &file, &digest).is_err());
    assert!(extract_verified_archive(&archive, &dir, &digest).is_err());
    assert_eq!(fs::read(&file).unwrap(), b"existing file");
    assert_eq!(fs::read(&kept).unwrap(), b"existing directory contents");
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn extraction_passes_use_the_open_archive_even_after_its_name_changes() {
    use super::{extract_open_file, verify_open_file};

    let (_temp, root) = canonical_tempdir();
    let src = root.join("mod.zip");
    let expected = write_archive(&src);
    let mut input = crate::safe_fs::open_regular(&src).unwrap();
    fs::rename(&src, root.join("original.zip")).unwrap();
    fs::write(&src, b"replacement at the original name").unwrap();
    let dest = root.join("staged");

    let (listing, written) = extract_open_file(&mut input, "zip", &dest, None).unwrap();
    verify_open_file(&mut input, &expected).unwrap();
    assert_eq!(listing.total_size, 5);
    assert_eq!(written.len(), 2);
    assert_eq!(
        fs::read(dest.join("Wrapper/data/hello.txt")).unwrap(),
        b"hello"
    );
    assert_eq!(fs::read(src).unwrap(), b"replacement at the original name");
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn staging_handle_rejects_a_nested_link_and_detects_a_replaced_root() {
    use crate::safe_fs::StagingDirectory;

    let (_temp, root) = canonical_tempdir();
    let dest = root.join("staged");
    let outside = root.join("unrelated");
    fs::create_dir(&outside).unwrap();
    let stage = StagingDirectory::create(&dest).unwrap();
    stage.create_dir_all(Path::new("existing")).unwrap();
    symlink(&outside, dest.join("existing/link")).unwrap();

    assert!(stage
        .create_file(Path::new("existing/link/escaped"))
        .is_err());
    assert!(stage
        .create_dir_all(Path::new("existing/link/escaped-dir"))
        .is_err());
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    stage.verify_path().unwrap();

    fs::rename(&dest, root.join("original-stage")).unwrap();
    fs::create_dir(&dest).unwrap();
    assert!(stage.verify_path().is_err());
    assert!(fs::read_dir(&dest).unwrap().next().is_none());
}
