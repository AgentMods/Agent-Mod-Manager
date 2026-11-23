use crate::archive::{extract_archive, list_archive, verify_archive_members};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

const FIXTURE_DIR: &str = "tests/fixtures/archive-rar";
const RAR4_SIGNATURE: &[u8] = b"Rar!\x1a\x07\x00";
const RAR5_SIGNATURE: &[u8] = b"Rar!\x1a\x07\x01\x00";

#[derive(Clone, Copy)]
struct ExpectedEntry {
    path: &'static str,
    is_dir: bool,
    size: u64,
}

#[derive(Clone, Copy)]
struct Fixture {
    name: &'static str,
    filename: &'static str,
    signature: &'static [u8],
    entries: &'static [ExpectedEntry],
    sha256: &'static str,
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "legacy RAR4",
        filename: "rar4-legacy.rar",
        signature: RAR4_SIGNATURE,
        entries: &[ExpectedEntry {
            path: "testfile.txt",
            is_dir: false,
            size: 12,
        }],
        sha256: "dce342bc0c2852fcaa36a03da5e55abb7dd69c045bbd812faebebc1a3844f5a4",
    },
    Fixture {
        name: "RAR5 mixed methods",
        filename: "rar5-mixed.rar",
        signature: RAR5_SIGNATURE,
        entries: &[
            ExpectedEntry {
                path: "testfile.jpg",
                is_dir: false,
                size: 220,
            },
            ExpectedEntry {
                path: "testfile.png",
                is_dir: false,
                size: 87,
            },
        ],
        sha256: "e8b106048f18e6fb9a5f8ec6a95346e76906e7e4e9ca15ec97e4f926159cb398",
    },
    Fixture {
        name: "RAR5 nested directory",
        filename: "rar5-nested.rar",
        signature: RAR5_SIGNATURE,
        entries: &[
            ExpectedEntry {
                path: "payload/inner.zip",
                is_dir: false,
                size: 165,
            },
            ExpectedEntry {
                path: "payload/real_after.txt",
                is_dir: false,
                size: 21,
            },
            ExpectedEntry {
                path: "payload",
                is_dir: true,
                size: 0,
            },
        ],
        sha256: "4d0735b083c85575d19585cb2ae629267a046689b3ecacac6c1ee03902edef86",
    },
];

fn fixture_path(filename: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(FIXTURE_DIR)
        .join(filename)
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut input = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn member_path(path: &str) -> PathBuf {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .fold(PathBuf::new(), |mut result, part| {
            result.push(part);
            result
        })
}

fn normalized_member_name(path: &str) -> String {
    path.replace('\\', "/")
        .trim_end_matches(['/', '\\'])
        .to_owned()
}

fn expected_listing_entries(fixture: Fixture) -> BTreeSet<(String, bool, u64)> {
    fixture
        .entries
        .iter()
        .map(|entry| (entry.path.to_owned(), entry.is_dir, entry.size))
        .collect()
}

fn listed_entries(listing: &crate::archive::ArchiveListing) -> BTreeSet<(String, bool, u64)> {
    listing
        .entries
        .iter()
        .map(|entry| {
            (
                normalized_member_name(&entry.path),
                entry.is_dir,
                entry.size,
            )
        })
        .collect()
}

#[test]
fn ordinary_rar4_and_rar5_fixtures_roundtrip_through_list_verify_extract() {
    for fixture in FIXTURES {
        let archive = fixture_path(fixture.filename);
        let mut input = File::open(&archive).expect("fixture exists");
        let mut signature = vec![0; fixture.signature.len()];
        input
            .read_exact(&mut signature)
            .expect("fixture has a signature");
        assert_eq!(signature, fixture.signature, "{}", fixture.name);
        assert_eq!(
            sha256_file(&archive).unwrap(),
            fixture.sha256,
            "fixture bytes changed: {}",
            fixture.name
        );

        let listing = list_archive(&archive).expect("list valid RAR fixture");
        assert_eq!(listing.format, "rar", "{}", fixture.name);
        assert_eq!(
            listing.entries.len(),
            fixture.entries.len(),
            "{}",
            fixture.name
        );
        assert_eq!(listed_entries(&listing), expected_listing_entries(*fixture));
        let file_count = listing.entries.iter().filter(|entry| !entry.is_dir).count();
        assert_eq!(
            listing.total_size,
            fixture
                .entries
                .iter()
                .filter(|entry| !entry.is_dir)
                .map(|entry| entry.size)
                .sum::<u64>(),
            "{}",
            fixture.name
        );

        let expected_archive_hash = fixture.sha256;
        let capture_path = member_path(fixture.entries[0].path);
        let capture = BTreeMap::from([(capture_path.clone(), 8usize)]);
        let verified = verify_archive_members(
            &archive,
            expected_archive_hash,
            &capture,
            &CancellationToken::new(),
        )
        .expect("verify valid RAR fixture");
        assert_eq!(verified.listing.format, "rar", "{}", fixture.name);
        assert_eq!(
            verified.listing.entries, listing.entries,
            "{}",
            fixture.name
        );
        assert_eq!(
            verified.listing.total_size, listing.total_size,
            "{}",
            fixture.name
        );
        assert_eq!(verified.hashes.len(), file_count, "{}", fixture.name);
        assert_eq!(verified.captured.len(), 1, "{}", fixture.name);

        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("extracted");
        assert!(!destination.exists());
        let written = extract_archive(&archive, &destination, None)
            .expect("extract valid RAR fixture into new staging directory");
        assert_eq!(written.len(), file_count, "{}", fixture.name);

        let listed_files = listing
            .entries
            .iter()
            .filter(|entry| !entry.is_dir)
            .map(|entry| member_path(&entry.path))
            .collect::<BTreeSet<_>>();
        let written_files = written
            .iter()
            .map(|path| {
                path.strip_prefix(&destination)
                    .expect("extracted paths stay under the destination")
                    .to_path_buf()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(written_files, listed_files, "{}", fixture.name);

        for relative in &listed_files {
            let extracted = destination.join(relative);
            assert!(extracted.is_file(), "{}", extracted.display());
            let extracted_hash = sha256_file(&extracted).unwrap();
            assert_eq!(
                verified.hashes.get(relative),
                Some(&extracted_hash),
                "SHA256 mismatch for {} in {}",
                relative.display(),
                fixture.name
            );
        }
        let captured = verified
            .captured
            .get(&capture_path)
            .expect("requested prefix was captured");
        let captured_source = fs::read(destination.join(&capture_path)).unwrap();
        assert_eq!(
            captured.prefix,
            captured_source[..captured.prefix.len()],
            "captured prefix differs from extracted file in {}",
            fixture.name
        );
        assert_eq!(
            captured.truncated,
            captured_source.len() > 8,
            "{}",
            fixture.name
        );

        if fixture.filename == "rar4-legacy.rar" {
            assert_eq!(
                fs::read(destination.join("testfile.txt")).unwrap(),
                b"Testing 123\n"
            );
        }
        if fixture.filename == "rar5-mixed.rar" {
            assert!(fs::read(destination.join("testfile.jpg"))
                .unwrap()
                .starts_with(&[0xff, 0xd8, 0xff]));
            assert!(fs::read(destination.join("testfile.png"))
                .unwrap()
                .starts_with(b"\x89PNG\r\n\x1a\n"));
        }
        if fixture.filename == "rar5-nested.rar" {
            assert!(fs::read(destination.join("payload/inner.zip"))
                .unwrap()
                .starts_with(b"PK\x03\x04"));
        }
    }
}

fn assert_rejected_without_extracting(archive: &Path) {
    assert!(list_archive(archive).is_err(), "{}", archive.display());

    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("must-not-be-created");
    assert!(extract_archive(archive, &destination, None).is_err());
    assert!(
        !destination.exists(),
        "rejected archive created {}",
        destination.display()
    );

    let digest = sha256_file(archive).unwrap();
    assert!(verify_archive_members(
        archive,
        &digest,
        &BTreeMap::new(),
        &CancellationToken::new(),
    )
    .is_err());
    assert!(
        !destination.exists(),
        "verification created {}",
        destination.display()
    );
}

#[test]
fn truncated_rar4_and_rar5_archives_are_rejected_without_writes() {
    for fixture in FIXTURES {
        let source = fixture_path(fixture.filename);
        let mut bytes = fs::read(source).unwrap();
        bytes.truncate(fixture.signature.len() + 2);

        let temp = tempfile::tempdir().unwrap();
        let damaged = temp.path().join("truncated.rar");
        fs::write(&damaged, bytes).unwrap();
        assert_rejected_without_extracting(&damaged);
    }
}

#[test]
fn fake_rar4_and_rar5_signatures_are_rejected_without_writes() {
    for (name, fake) in [
        (
            "fake-rar4.rar",
            b"Rar!\x1a\x07\x00not-a-rar-archive".as_slice(),
        ),
        (
            "fake-rar5.rar",
            b"Rar!\x1a\x07\x01\x00not-a-rar-archive".as_slice(),
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join(name);
        fs::write(&archive, fake).unwrap();
        assert_rejected_without_extracting(&archive);
    }
}

#[test]
fn encrypted_rar4_and_rar5_are_rejected_without_partial_writes() {
    for (filename, expected_sha256) in [
        (
            "rar4-encrypted.rar",
            "c30dc6d10e59aaab1c863afa469d188c644a22d67d6e50ac7ca887e520f5fc8b",
        ),
        (
            "rar5-encrypted.rar",
            "72b69a1d5f5757511e683a9207166c60e91ce0846ae446b8b332b085f9e0ba48",
        ),
    ] {
        let archive = fixture_path(filename);
        assert_eq!(
            sha256_file(&archive).unwrap(),
            expected_sha256,
            "{filename}"
        );
        assert_rejected_without_extracting(&archive);
    }
}

#[test]
fn verification_rejects_wrong_archive_digest_and_over_budget_capture() {
    let archive = fixture_path("rar5-mixed.rar");
    let digest = sha256_file(&archive).unwrap();
    let wrong_digest = "0".repeat(64);
    assert_ne!(digest, wrong_digest);

    assert!(verify_archive_members(
        &archive,
        &wrong_digest,
        &BTreeMap::new(),
        &CancellationToken::new(),
    )
    .is_err());

    let oversized_capture = BTreeMap::from([(PathBuf::from("testfile.jpg"), 512 * 1024 + 1)]);
    assert!(verify_archive_members(
        &archive,
        &digest,
        &oversized_capture,
        &CancellationToken::new(),
    )
    .is_err());

    let too_many_captures = (0..9)
        .map(|index| (PathBuf::from(format!("member-{index}.txt")), 1usize))
        .collect::<BTreeMap<_, _>>();
    assert!(verify_archive_members(
        &archive,
        &digest,
        &too_many_captures,
        &CancellationToken::new(),
    )
    .is_err());
    assert_eq!(sha256_file(&archive).unwrap(), digest);
}

/// Opt-in check for a caller-supplied local archive. The source is only opened
/// through the read-only archive APIs; extraction goes to a fresh temp folder.
#[test]
#[ignore = "opt in with AMM_RAR_SAMPLE set to an absolute local RAR path"]
fn rar_sample_from_environment() {
    let archive = PathBuf::from(
        std::env::var_os("AMM_RAR_SAMPLE").expect("set AMM_RAR_SAMPLE to an absolute RAR path"),
    );
    assert!(
        archive.is_absolute(),
        "AMM_RAR_SAMPLE must be an absolute path"
    );
    assert!(archive.is_file(), "AMM_RAR_SAMPLE must name a regular file");

    let source_hash = sha256_file(&archive).expect("read sample SHA256");
    let listing = list_archive(&archive).expect("list local RAR sample");
    assert_eq!(listing.format, "rar");
    let expected_paths = listing
        .entries
        .iter()
        .filter(|entry| !entry.is_dir)
        .map(|entry| member_path(&entry.path))
        .collect::<BTreeSet<_>>();
    let expected_file_count = expected_paths.len();
    let verified = verify_archive_members(
        &archive,
        &source_hash,
        &BTreeMap::new(),
        &CancellationToken::new(),
    )
    .expect("verify local RAR sample");
    assert_eq!(verified.listing.format, "rar");
    assert_eq!(verified.listing.entries, listing.entries);
    assert_eq!(verified.listing.total_size, listing.total_size);
    assert_eq!(verified.hashes.len(), expected_file_count);

    let temp = tempfile::tempdir().expect("create an isolated temporary directory");
    let destination = temp.path().join("rar-sample-extracted");
    assert!(!destination.exists());
    let written = extract_archive(&archive, &destination, None).expect("extract local RAR sample");
    assert_eq!(written.len(), expected_file_count);
    let written_paths = written
        .iter()
        .map(|path| {
            path.strip_prefix(&destination)
                .expect("extracted path is contained by the temp directory")
                .to_path_buf()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(written_paths, expected_paths);

    let mut actual_total_size = 0u64;
    for relative in &expected_paths {
        let extracted = destination.join(relative);
        assert!(fs::metadata(&extracted).unwrap().is_file());
        let size = fs::metadata(&extracted).unwrap().len();
        let entry = listing
            .entries
            .iter()
            .find(|entry| !entry.is_dir && member_path(&entry.path) == *relative)
            .expect("listed file exists");
        assert_eq!(size, entry.size, "{}", relative.display());
        actual_total_size = actual_total_size
            .checked_add(size)
            .expect("sample total size fits in u64");

        let extracted_hash = sha256_file(&extracted).unwrap();
        assert_eq!(
            verified.hashes.get(relative),
            Some(&extracted_hash),
            "SHA256 mismatch for {}",
            relative.display()
        );
    }
    assert_eq!(actual_total_size, listing.total_size);
    assert_eq!(
        sha256_file(&archive).unwrap(),
        source_hash,
        "sample input changed"
    );
    println!(
        "RAR sample verified: {} entries ({expected_file_count} files), {actual_total_size} bytes",
        listing.entries.len()
    );
}
