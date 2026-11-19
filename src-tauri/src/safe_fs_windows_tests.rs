use super::{
    create_dir_all, create_new, directory_entries, identity, open_regular, rename_checked,
    rename_exclusive, StagingDirectory,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn temp_root() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    assert!(root.is_absolute());
    (temp, root)
}

fn create_junction(link: &Path, target: &Path) {
    let parent = link.parent().expect("junction path must have a parent");
    assert_eq!(target.parent(), Some(parent));
    let output = Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(link.file_name().unwrap())
        .arg(target.file_name().unwrap())
        .current_dir(parent)
        .output()
        .expect("Windows CI must provide cmd.exe for the junction fixture");
    assert!(
        output.status.success(),
        "mklink /J fixture failed ({}): {}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn checked_renames_round_trip_within_and_across_parent_handles() {
    let (_temp, root) = temp_root();
    let source = root.join("source.bin");
    fs::write(&source, b"same file").unwrap();
    let same_parent = root.join("renamed.bin");
    let other_dir = root.join("other-parent");
    fs::create_dir(&other_dir).unwrap();
    let other_parent = other_dir.join("x");
    let expected = identity(&source).unwrap().unwrap();
    rename_checked(&source, &same_parent, &expected).unwrap();
    rename_checked(&same_parent, &other_parent, &expected).unwrap();
    assert!(!source.exists());
    assert!(!same_parent.exists());
    assert_eq!(identity(&other_parent).unwrap(), Some(expected));
    assert_eq!(fs::read(other_parent).unwrap(), b"same file");
}

#[test]
fn operations_reject_junction_ancestors_without_touching_target() {
    let (_temp, root) = temp_root();
    let target = root.join("junction-target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("read.bin"), b"read fixture").unwrap();
    fs::write(target.join("source.bin"), b"rename fixture").unwrap();

    let junction = root.join("junction");
    create_junction(&junction, &target);
    let id = identity(&root).unwrap().unwrap();
    let entries = directory_entries(&root, &id)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(entries.contains(&("junction-target".into(), true)));
    assert!(entries.contains(&("junction".into(), false)));

    let read_path = junction.join("read.bin");
    assert!(open_regular(&read_path).is_err());
    assert!(identity(&read_path).is_err());

    assert!(create_new(&junction.join("created.bin")).is_err());
    assert!(create_dir_all(&junction.join("new-parent").join("child")).is_err());

    let rename_source = root.join("rename-source.bin");
    let rename_destination = junction.join("renamed.bin");
    fs::write(&rename_source, b"source remains").unwrap();
    assert!(rename_exclusive(&rename_source, &rename_destination).is_err());
    assert_eq!(fs::read(&rename_source).unwrap(), b"source remains");

    let moved_from_junction = root.join("moved-from-junction.bin");
    assert!(rename_exclusive(&junction.join("source.bin"), &moved_from_junction).is_err());
    assert_eq!(
        fs::read(target.join("source.bin")).unwrap(),
        b"rename fixture"
    );

    assert!(!target.join("created.bin").exists());
    assert!(!target.join("new-parent").exists());
    assert!(!target.join("renamed.bin").exists());
    assert!(!moved_from_junction.exists());
}

#[test]
fn rename_checks_expected_identity_and_never_clobbers_destination() {
    let (_temp, root) = temp_root();
    let source = root.join("source.bin");
    let destination = root.join("destination.bin");
    fs::write(&source, b"source").unwrap();
    fs::write(&destination, b"existing destination").unwrap();

    assert!(rename_exclusive(&source, &destination).is_err());
    assert_eq!(fs::read(&source).unwrap(), b"source");
    assert_eq!(fs::read(&destination).unwrap(), b"existing destination");

    let expected = identity(&source).unwrap().unwrap();
    let old_source = root.join("old-source.bin");
    fs::rename(&source, &old_source).unwrap();
    fs::write(&source, b"source").unwrap();
    let checked_destination = root.join("checked-destination.bin");

    assert!(rename_checked(&source, &checked_destination, &expected).is_err());
    assert_eq!(fs::read(&source).unwrap(), b"source");
    assert_eq!(fs::read(&old_source).unwrap(), b"source");
    assert!(!checked_destination.exists());
}

#[test]
fn staging_directory_rejects_root_replacement_while_live() {
    let (_temp, root) = temp_root();
    let stage_path = root.join("staging");
    let backup_path = root.join("staging-backup");
    let stage = StagingDirectory::create(&stage_path).unwrap();

    match fs::rename(&stage_path, &backup_path) {
        Ok(()) => {
            fs::create_dir(&stage_path).unwrap();
            assert!(stage.verify_path().is_err());
            assert!(stage.create_file(Path::new("escaped.bin")).is_err());
            assert!(!backup_path.join("escaped.bin").exists());
            assert!(!stage_path.join("escaped.bin").exists());
        }
        Err(error) => {
            assert!(
                matches!(error.raw_os_error(), Some(5 | 32 | 33)),
                "unexpected staging-root rename failure: {error}"
            );
            stage.verify_path().unwrap();
            drop(stage.create_file(Path::new("still-pinned.bin")).unwrap());
            assert!(stage_path.join("still-pinned.bin").is_file());
            assert!(!backup_path.exists());
        }
    }
}

#[test]
fn staging_pins_its_ancestors_and_validates_the_original_entry_spelling() {
    use std::io::Write;

    let (_temp, root) = temp_root();
    let outer = root.join("staging-parent");
    fs::create_dir(&outer).unwrap();
    let stage_path = outer.join("staging");
    let stage = StagingDirectory::create(&stage_path).unwrap();
    assert!(fs::rename(&outer, root.join("moved-parent")).is_err());
    assert!(stage.create_file(Path::new(r"payload\.")).is_err());
    assert!(!stage_path.join("payload").exists());
    let mut file = stage.create_file(Path::new(r"nested\payload")).unwrap();
    file.write_all(b"fixture").unwrap();
    drop(file);
    stage.verify_path().unwrap();
    assert_eq!(
        fs::read(stage_path.join(r"nested\payload")).unwrap(),
        b"fixture"
    );
}
