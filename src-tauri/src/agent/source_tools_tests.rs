use super::*;
use crate::{agent::policy::PermissionMode, settings::SecretStore, transaction::tests::setup};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Secrets {
    values: Mutex<BTreeMap<String, String>>,
    reads: AtomicUsize,
}
impl SecretStore for Secrets {
    fn read(&self, key: &str) -> AppResult<Option<String>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.values.lock().unwrap().get(key).cloned())
    }
    fn write(&self, key: &str, value: &str) -> AppResult<()> {
        self.values.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
}

fn fixture() -> (tempfile::TempDir, Settings, ToolContext, Arc<Secrets>) {
    let (dir, engine, _) = setup();
    let secrets = Arc::new(Secrets::default());
    let settings = Settings::with_secret_store(engine.db.clone(), secrets.clone());
    let context = ToolContext {
        engine,
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Agent("fixture".into()),
    };
    (dir, settings, context, secrets)
}

fn three_dm_status(settings: &Settings) -> crate::sources::SourceStatus {
    sources(settings)
        .unwrap()
        .statuses()
        .into_iter()
        .find(|s| s.source_id == "3dm")
        .unwrap()
}

#[tokio::test]
async fn download_cannot_swap_resolutions_after_preview() {
    use crate::sources::{FileHashes, ResolveResult, ResolvedMod, SourceState, SourceStatus};

    let (_dir, settings, context, _) = fixture();
    let registry = ToolRegistry::default();
    let store = Resolutions::default();
    register(&registry, settings, store.clone()).unwrap();
    let tool = registry.get("download_mod").unwrap();
    let ids = ["Version1", "Version2"].map(|version| {
        store
            .publish(
                context.resolution_scope.clone(),
                ResolveResult {
                    status: SourceStatus::new("modrinth", SourceState::Available, "OK"),
                    download: Some(ResolvedMod {
                        source_id: "modrinth".into(),
                        project_id: "Project1".into(),
                        version_id: version.into(),
                        name: "Fixture".into(),
                        version: version.into(),
                        download_url: "https://cdn.example.com/mod.jar?secret=private".into(),
                        filename: "mod.jar".into(),
                        size: Some(20),
                        hashes: FileHashes::default(),
                        homepage_url: None,
                        warnings: Vec::new(),
                    }),
                    homepage_url: None,
                    dependencies: None,
                },
            )
            .unwrap()
            .download
            .unwrap()
            .resolution_id
    });
    let prepared = tool
        .prepare(
            &context,
            &json!({"resolution_id":ids[0],"gameName":"Example Game"}),
        )
        .await
        .unwrap();
    assert_eq!(prepared.preview["filename"], "mod.jar");
    assert_eq!(prepared.preview["version_id"], "Version1");
    assert!(!prepared.preview.to_string().contains("secret"));
    assert!(matches!(
        tool.execute(
            &context,
            json!({"resolution_id":ids[1],"gameName":"Example Game"}),
            prepared
        )
        .await,
        Err(AppError::Denied(_))
    ));
    let args = json!({"resolution_id":ids[0],"gameName":"Example Game"});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    let mut changed = args;
    changed["gameName"] = json!("Other Game");
    assert!(matches!(
        tool.execute(&context, changed, prepared).await,
        Err(AppError::Denied(_))
    ));
    assert!(tool
        .prepare(&context, &json!({"resolution_id":ids[0]}))
        .await
        .is_err());
    for id in ids {
        assert!(store.preview(&context.resolution_scope, id).is_ok());
    }
    assert!(crate::artifacts::list(&context.engine.db)
        .unwrap()
        .is_empty());
}

#[test]
fn source_keys_reload_and_never_borrow_mcp_credentials() {
    use crate::sources::SourceState;
    let (_dir, settings, _, _) = fixture();
    settings
        .write_secret("mcp.glossmod_3dm.api_key", "mcp-private-test-value")
        .unwrap();
    assert_eq!(three_dm_status(&settings).state, SourceState::MissingKey);
    settings
        .write_secret("sources.3dm", "source-private-test-value")
        .unwrap();
    assert_eq!(three_dm_status(&settings).state, SourceState::Limited);
    settings.write_secret("sources.3dm", "bad\nheader").unwrap();
    let statuses = sources(&settings).unwrap().statuses();
    assert_eq!(
        statuses
            .iter()
            .find(|s| s.source_id == "3dm")
            .unwrap()
            .state,
        SourceState::InvalidKey
    );
    assert_eq!(
        statuses
            .iter()
            .find(|s| s.source_id == "modrinth")
            .unwrap()
            .state,
        SourceState::Available
    );
    let encoded = serde_json::to_string(&statuses).unwrap();
    assert!(!encoded.contains("private-test-value") && !encoded.contains("bad\\nheader"));
    settings.write_secret("sources.3dm", " ").unwrap();
    assert_eq!(three_dm_status(&settings).state, SourceState::MissingKey);
}

#[tokio::test]
async fn agent_source_tools_keep_read_and_download_permissions_separate() {
    let (_dir, settings, context, _) = fixture();
    let registry = ToolRegistry::default();
    register(&registry, settings, Resolutions::default()).unwrap();
    for name in [
        "search_mods",
        "list_mod_versions",
        "resolve_mod",
        "resolve_modrinth_dependency_graph",
    ] {
        assert_eq!(
            registry.get(name).unwrap().definition().effect,
            Effect::Read
        );
    }
    let download = registry.get("download_mod").unwrap();
    assert_eq!(download.definition().effect, Effect::ManagerWrite);
    assert!(PermissionMode::RequestApproval.needs_approval(download.definition().effect));
    let search = registry.get("search_mods").unwrap();
    let args = json!({"query":"sample", "source_ids":["3dm"]});
    let prepared = search.prepare(&context, &args).await.unwrap();
    let result = search.execute(&context, args, prepared).await.unwrap();
    assert_eq!(result["sources"][0]["source_id"], "3dm");
    assert_eq!(result["sources"][0]["state"], "missing_key");
    assert_eq!(result["mods"], json!([]));
    assert!(
        context
            .engine
            .db
            .conn()
            .query_row("SELECT COUNT(*) FROM file_transactions", [], |row| row
                .get::<_, u64>(0))
            .unwrap()
            == 0
    );
}

#[tokio::test]
async fn dependency_graph_rejects_local_fields_and_never_needs_source_secrets() {
    let (_dir, settings, context, secrets) = fixture();
    settings
        .write_secret("sources.3dm", "private-source-test-value")
        .unwrap();
    let registry = ToolRegistry::default();
    register(&registry, settings, Resolutions::default()).unwrap();
    let tool = registry.get("resolve_modrinth_dependency_graph").unwrap();
    let definition = tool.definition();
    let pins = &definition.parameters["properties"]["project_pins"];
    assert!(pins.get("maxProperties").is_none());
    assert_eq!(pins["propertyNames"]["pattern"], "^[A-Za-z0-9]{1,32}$");
    for args in [
        json!({"version_id":"abcd1234","loader":"fabric"}),
        json!({"version_id":"abcd1234","game_version":"1.21","loader":"fabric","game_id":"local"}),
        json!({"version_id":"abcd1234","game_version":"1.21","loader":"fabric","project_pins":{"../outside":"abcd1234"}}),
        json!({"version_id":"abcd1234","game_version":"/Users/private/path","loader":"fabric"}),
    ] {
        let prepared = tool.prepare(&context, &args).await.unwrap();
        assert_eq!(prepared.effect, Effect::Read);
        assert!(prepared.plan.is_none());
        assert!(tool.execute(&context, args, prepared).await.is_err());
    }
    context.cancel.cancel();
    let args = json!({"version_id":"abcd1234","game_version":"1.21","loader":"fabric"});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert!(matches!(
        tool.execute(&context, args, prepared).await,
        Err(AppError::Cancelled)
    ));
    assert_eq!(secrets.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn version_list_degrades_unconfigured_sources_without_reading_source_secrets() {
    let (_dir, settings, context, secrets) = fixture();
    settings
        .write_secret("sources.3dm", "private-source-test-value")
        .unwrap();
    let registry = ToolRegistry::default();
    register(&registry, settings, Resolutions::default()).unwrap();
    let tool = registry.get("list_mod_versions").unwrap();
    for source_id in ["curseforge", "nexus", "3dm"] {
        let args = json!({"source_id":source_id,"project_id":"123","game_version":null});
        let prepared = tool.prepare(&context, &args).await.unwrap();
        assert_eq!(prepared.effect, Effect::Read);
        assert!(!PermissionMode::RequestApproval.needs_approval(prepared.effect));
        assert!(prepared.plan.is_none());
        let result = tool.execute(&context, args, prepared).await.unwrap();
        assert_eq!(result["status"]["source_id"], source_id);
        assert_eq!(result["status"]["state"], "unsupported");
        assert_eq!(result["versions"], json!([]));
        assert_eq!(result["has_more"], false);
    }
    let args = json!({
        "source_id":"thunderstore",
        "project_id":"Team/SampleMod",
        "game_id":null,
        "game_version":null
    });
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert_eq!(prepared.effect, Effect::Read);
    assert!(!PermissionMode::RequestApproval.needs_approval(prepared.effect));
    let result = tool.execute(&context, args, prepared).await.unwrap();
    assert_eq!(result["status"]["source_id"], "thunderstore");
    assert_eq!(result["status"]["state"], "needs_configuration");
    assert_eq!(result["versions"], json!([]));
    assert_eq!(result["has_more"], false);
    let unknown = json!({"source_id":"unknown","project_id":"123"});
    let prepared = tool.prepare(&context, &unknown).await.unwrap();
    assert!(matches!(
        tool.execute(&context, unknown, prepared).await,
        Err(AppError::Unsupported(_))
    ));
    assert_eq!(secrets.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn version_list_enforces_flat_arguments_and_honors_cancellation() {
    let (_dir, settings, context, secrets) = fixture();
    let tool = SourceTool {
        name: "list_mod_versions".into(),
        settings,
        resolutions: Resolutions::default(),
    };
    assert_eq!(
        tool.definition().parameters,
        json!({
            "type":"object",
            "properties":{
                "source_id":{"type":"string"},
                "project_id":{"type":"string"},
                "game_id":{"type":["string","null"]},
                "game_version":{"type":["string","null"]},
                "loader":{"type":["string","null"]},
                "offset":{"type":"integer","minimum":0,"default":0},
                "limit":{"type":"integer","minimum":1,"maximum":50,"default":20}
            },
            "required":["source_id","project_id"],
            "additionalProperties":false
        })
    );
    let base = json!({"source_id":"thunderstore","project_id":"123"});
    let mut invalid_args = vec![
        json!({"source_id":"thunderstore"}),
        json!({"project_id":"123"}),
        json!({"request":base}),
        json!(["thunderstore", "123", null, null, null, 0, 20]),
    ];
    for (key, value) in [
        ("game_id", json!(42)),
        ("game_version", json!(1.21)),
        ("loader", json!(["fabric"])),
        ("offset", json!(-1)),
        ("limit", json!(0)),
        ("limit", json!(51)),
        ("limit", json!(1.5)),
        ("version_id", json!("release1")),
    ] {
        let mut args = base.clone();
        args[key] = value;
        invalid_args.push(args);
    }
    for args in invalid_args {
        let prepared = tool.prepare(&context, &args).await.unwrap();
        assert!(
            tool.execute(&context, args.clone(), prepared)
                .await
                .is_err(),
            "accepted invalid args: {args}"
        );
    }
    context.cancel.cancel();
    let args = json!({"source_id":"modrinth","project_id":"sample123"});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert!(matches!(
        tool.execute(&context, args, prepared).await,
        Err(AppError::Cancelled)
    ));
    assert_eq!(secrets.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancellation_and_malformed_arguments_never_read_source_secrets() {
    let (_dir, settings, context, secrets) = fixture();
    settings
        .write_secret("sources.3dm", "private-source-test-value")
        .unwrap();
    let tool = SourceTool {
        name: "search_mods".into(),
        settings,
        resolutions: Resolutions::default(),
    };
    let args = json!({"query":"sample", "source_ids":["3dm"], "unexpected":true});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert!(tool.execute(&context, args, prepared).await.is_err());
    assert_eq!(secrets.reads.load(Ordering::SeqCst), 0);
    context.cancel.cancel();
    let args = json!({"query":"sample", "source_ids":["3dm"]});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert!(matches!(
        tool.execute(&context, args, prepared).await,
        Err(AppError::Cancelled)
    ));
    assert_eq!(secrets.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_download_retry_is_single_receipt_bound_and_legacy_cannot_be_adopted() {
    use crate::{
        artifacts,
        download::DownloadArtifact,
        mod_library::{DownloadMetadata, NativeDownloadIntent},
    };
    use sha2::{Digest, Sha256, Sha512};
    use std::fs;

    let (_dir, settings, mut context, secret_store) = fixture();
    let registry = ToolRegistry::default();
    register(&registry, settings, Resolutions::default()).unwrap();
    let tool = registry.get(RETRY).unwrap();
    assert_eq!(tool.definition().effect, Effect::ManagerWrite);
    assert!(PermissionMode::RequestApproval.needs_approval(tool.definition().effect));
    assert!(PermissionMode::AutoApprove.needs_approval(tool.definition().effect));
    assert_eq!(
        tool.definition().parameters["required"],
        json!(["artifactId"])
    );
    let cache = context.engine.paths.downloads().join("0".repeat(64));
    fs::create_dir(&cache).unwrap();
    let bytes = b"fixture transport bytes";
    let create_artifact = || {
        let id = uuid::Uuid::new_v4().to_string();
        let filename = "mod.zip".to_string();
        let path = cache.join(format!("{id}-{filename}"));
        fs::write(&path, bytes).unwrap();
        DownloadArtifact {
            id,
            path,
            filename,
            size: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(bytes)),
            sha512: hex::encode(Sha512::digest(bytes)),
            integrity_verified: false,
            requires_review: true,
            auto_install_allowed: false,
            warnings: Vec::new(),
        }
    };
    let original = create_artifact();
    let other = create_artifact();
    let legacy = create_artifact();
    let intent = NativeDownloadIntent::new(
        "Saved Game",
        None,
        DownloadMetadata {
            project_page: Some("https://example.com/author-project"),
            author_hint: Some("Public Author"),
        },
    )
    .unwrap();
    for saved in [&original, &other] {
        artifacts::record_web_with_library_intent(&context.engine, saved, &intent).unwrap();
    }
    artifacts::record_web(&context.engine, &legacy).unwrap();
    let args = json!({"artifactId":original.id,"gameName":"Saved Game"});
    for invalid in [
        json!({"artifactId":original.id}),
        json!({"artifactId":original.id,"gameName":"Wrong Game"}),
        json!({"artifactId":legacy.id,"gameName":"Saved Game"}),
        json!({"artifactId":[original.id,other.id],"gameName":"Saved Game"}),
        json!({"artifactId":original.id,"gameName":"Saved Game","projectPage":"https://example.com/replacement"}),
    ] {
        assert!(tool.prepare(&context, &invalid).await.is_err(), "{invalid}");
    }
    let mut mcp = context.clone();
    mcp.resolution_scope = ResolutionScope::Mcp("fixture".into());
    assert!(tool.prepare(&mcp, &args).await.is_err());
    let approved = tool.prepare(&context, &args).await.unwrap();
    assert_eq!(approved.preview["artifactId"], original.id);
    assert_eq!(approved.preview["sha256"], original.sha256);
    assert!(matches!(
        tool.execute(
            &context,
            json!({"artifactId":other.id,"gameName":"Saved Game"}),
            approved
        )
        .await,
        Err(AppError::Denied(_))
    ));
    let approved = tool.prepare(&context, &args).await.unwrap();
    context.cancel.cancel();
    assert!(matches!(
        tool.execute(&context, args.clone(), approved).await,
        Err(AppError::Cancelled)
    ));
    assert!(original.path.exists());
    assert_eq!(
        fs::read_dir(context.engine.paths.mod_library())
            .unwrap()
            .count(),
        0
    );
    context.cancel = CancellationToken::new();
    let approved = tool.prepare(&context, &args).await.unwrap();
    fs::write(&original.path, vec![b'x'; bytes.len()]).unwrap();
    assert!(tool
        .execute(&context, args.clone(), approved)
        .await
        .is_err());
    fs::write(&original.path, bytes).unwrap();
    let approved = tool.prepare(&context, &args).await.unwrap();
    let result = tool
        .execute(&context, args.clone(), approved)
        .await
        .unwrap();
    let library_file = std::path::Path::new(result["path"].as_str().unwrap());
    assert!(library_file
        .canonicalize()
        .unwrap()
        .starts_with(context.engine.paths.mod_library()));
    assert_eq!(fs::read(library_file).unwrap(), bytes);
    assert!(!original.path.exists());
    assert!(other.path.exists() && legacy.path.exists());
    assert_eq!(result["artifactId"], original.id);
    assert_eq!(result["projectPage"], "https://example.com/author-project");
    assert_eq!(result["authorHint"], "Public Author");
    assert_eq!(secret_store.reads.load(Ordering::SeqCst), 0);
}
