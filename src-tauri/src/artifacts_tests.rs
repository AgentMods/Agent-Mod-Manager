use super::*;
use crate::storage::AppPaths;
use sha2::{Digest, Sha256, Sha512};

const ARTIFACT_ID: &str = "d9428888-122b-4a57-8f72-3b1dc731d914";

fn fixture() -> (tempfile::TempDir, FileEngine, DownloadArtifact, ResolvedMod) {
    let temp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(temp.path().join("app")).unwrap();
    let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
    let cache = engine.paths.downloads().join("a".repeat(64));
    std::fs::create_dir(&cache).unwrap();
    let filename = "Example.zip";
    let path = cache.join(format!("{ARTIFACT_ID}-{filename}"));
    let data = b"test archive";
    std::fs::write(&path, data).unwrap();
    let artifact = DownloadArtifact {
        id: ARTIFACT_ID.into(),
        path,
        filename: filename.into(),
        size: data.len() as u64,
        sha256: hex::encode(Sha256::digest(data)),
        sha512: hex::encode(Sha512::digest(data)),
        integrity_verified: false,
        requires_review: true,
        auto_install_allowed: false,
        warnings: vec![],
    };
    let source = ResolvedMod {
        source_id: "thunderstore".into(),
        project_id: "Team_Name/Package-Name".into(),
        version_id: "1.2.3+build.1".into(),
        name: "Example".into(),
        version: "1.2.3".into(),
        download_url: "https://example.test/download?token=private-download".into(),
        filename: filename.into(),
        size: Some(artifact.size),
        hashes: Default::default(),
        homepage_url: Some("https://example.test/home?token=private-homepage".into()),
        warnings: vec![],
    };
    (temp, engine, artifact, source)
}

#[test]
fn exact_duplicate_preserves_catalog_data_and_ignores_request_urls() {
    let (_temp, engine, artifact, mut source) = fixture();
    record(&engine, &artifact, &source).unwrap();
    let created_at = "2020-01-01T00:00:00Z";
    let pretty_json = serde_json::to_string_pretty(&artifact).unwrap();
    engine
        .db
        .conn()
        .execute(
            "UPDATE download_artifacts SET created_at=?1, artifact_json=?2 WHERE id=?3",
            params![created_at, pretty_json, artifact.id],
        )
        .unwrap();
    source.download_url = "https://example.test/download?token=rotated-token".into();
    record(&engine, &artifact, &source).unwrap();

    let rows = list(&engine.db).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].created_at, created_at);
    assert_eq!(rows[0].artifact, artifact);
    let json = serde_json::to_string(&rows).unwrap();
    for secret in [
        "https://",
        "private-download",
        "private-homepage",
        "rotated-token",
    ] {
        assert!(!json.contains(secret));
    }
    let stored_json: String = engine
        .db
        .conn()
        .query_row("SELECT artifact_json FROM download_artifacts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(stored_json, pretty_json);
}

#[test]
fn catalog_conflicts_leave_the_original_entry_intact() {
    let (_temp, engine, artifact, source) = fixture();
    record(&engine, &artifact, &source).unwrap();
    assert!(matches!(
        record_web(&engine, &artifact),
        Err(AppError::Conflict(_))
    ));
    let other_path = artifact.path.with_file_name("Other.zip");
    std::fs::write(&other_path, b"test archive").unwrap();
    for changed in [
        DownloadArtifact {
            path: other_path.clone(),
            ..artifact.clone()
        },
        DownloadArtifact {
            sha256: "0".repeat(64),
            ..artifact.clone()
        },
        DownloadArtifact {
            size: artifact.size + 1,
            ..artifact.clone()
        },
        DownloadArtifact {
            filename: "Other.zip".into(),
            ..artifact.clone()
        },
        DownloadArtifact {
            id: uuid::Uuid::new_v4().to_string(),
            ..artifact.clone()
        },
    ] {
        let error = record(&engine, &changed, &source).unwrap_err();
        assert!(matches!(error, AppError::Conflict(_)));
        assert!(!error.to_string().contains(ARTIFACT_ID));
    }
    for (source_id, project_id, version_id) in [
        (
            "modrinth",
            source.project_id.as_str(),
            source.version_id.as_str(),
        ),
        (
            source.source_id.as_str(),
            "Other/Package",
            source.version_id.as_str(),
        ),
        (
            source.source_id.as_str(),
            source.project_id.as_str(),
            "2.0.0",
        ),
    ] {
        let changed = ResolvedMod {
            source_id: source_id.into(),
            project_id: project_id.into(),
            version_id: version_id.into(),
            ..source.clone()
        };
        assert!(matches!(
            record(&engine, &artifact, &changed),
            Err(AppError::Conflict(_))
        ));
    }
    assert!(for_path(&engine.db, &other_path).unwrap().is_none());
    let rows = list(&engine.db).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].artifact, artifact);
    assert_eq!(rows[0].source_id, source.source_id);
    assert_eq!(rows[0].project_id, source.project_id);
    assert_eq!(rows[0].version_id, source.version_id);
}

#[test]
fn web_catalog_is_immutable_and_never_exposes_provider_origin() {
    let (_temp, engine, artifact, source) = fixture();
    record_web(&engine, &artifact).unwrap();
    let first = for_path(&engine.db, &artifact.path).unwrap().unwrap();
    record_web(&engine, &artifact).unwrap();
    assert!(matches!(
        record(&engine, &artifact, &source),
        Err(AppError::Conflict(_))
    ));

    let mut stored = for_id(&engine.db, &artifact.id).unwrap().unwrap();
    assert_eq!(list(&engine.db).unwrap().len(), 1);
    assert_eq!(stored.created_at, first.created_at);
    assert_eq!(stored.artifact, artifact);
    assert_eq!(stored.source_id, "web");
    assert_eq!(stored.name, artifact.filename);
    assert!(stored.project_id.is_empty());
    assert!(stored.version_id.is_empty());
    assert!(stored.version.is_empty());
    assert!(stored.origin().is_none());

    stored.project_id = source.project_id;
    stored.version_id = source.version_id;
    assert!(stored.origin().is_none());
}

#[test]
fn web_catalog_rejects_payloads_outside_downloads() {
    let (_temp, engine, mut artifact, _) = fixture();
    let outside_downloads = engine.paths.staging().join(&artifact.filename);
    std::fs::rename(&artifact.path, &outside_downloads).unwrap();
    artifact.path = outside_downloads;
    assert!(matches!(
        record_web(&engine, &artifact),
        Err(AppError::PathEscape(_))
    ));
    assert!(list(&engine.db).unwrap().is_empty());
}

#[test]
fn origin_requires_a_canonical_file_id_and_complete_provider_ids() {
    let (_temp, engine, artifact, source) = fixture();
    record(&engine, &artifact, &source).unwrap();
    let row = for_path(&engine.db, &artifact.path).unwrap().unwrap();
    let origin = row.origin().unwrap();
    let json = serde_json::json!({
        "artifactId": ARTIFACT_ID,
        "sourceId": "thunderstore",
        "projectId": "Team_Name/Package-Name",
        "versionId": "1.2.3+build.1",
    });
    assert_eq!(serde_json::to_value(&origin).unwrap(), json);
    assert_eq!(
        serde_json::from_value::<DownloadOrigin>(json).unwrap(),
        origin
    );

    let mut changed = row.clone();
    changed.artifact.id = ARTIFACT_ID.to_ascii_uppercase();
    changed
        .artifact
        .path
        .set_file_name(format!("{}-Example.zip", changed.artifact.id));
    assert!(changed.origin().is_none());
    let mut changed = row.clone();
    changed.artifact.path.set_file_name("Example.zip");
    assert!(changed.origin().is_none());
    let too_long = "a".repeat(MAX_ORIGIN_ID_BYTES + 1);
    for invalid in [
        "",
        "  ",
        "name\n",
        "https://example.test/mod?token=private",
        "//example.test/mod",
        "name?token=private",
        too_long.as_str(),
    ] {
        for field in 0..3 {
            let mut changed = row.clone();
            [
                &mut changed.source_id,
                &mut changed.project_id,
                &mut changed.version_id,
            ][field]
                .replace_range(.., invalid);
            assert!(changed.origin().is_none());
        }
    }
}

#[test]
fn legacy_fingerprint_entries_remain_readable_without_exact_origin() {
    let (_temp, engine, mut artifact, source) = fixture();
    artifact.id = "a".repeat(64);
    record(&engine, &artifact, &source).unwrap();
    record(&engine, &artifact, &source).unwrap();
    let rows = list(&engine.db).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].artifact, artifact);
    let row = for_path(&engine.db, &artifact.path).unwrap().unwrap();
    assert_eq!(row.artifact, artifact);
    assert!(row.origin().is_none());
}
