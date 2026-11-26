use super::*;
use crate::{
    install::{self, LibraryInstallMode, LibraryInstallRequest, LibrarySourceKind, ManualMapping},
    mod_library::{self, ApprovedArtifact},
    mod_organization::list_mod_tags,
    settings::{AppSettings, SecretStore},
    transaction::FileEngine,
};
use axum::{body::Bytes, routing::post, Router};
use serde_json::Value;
use std::{fs, io::Write};
use tokio::sync::mpsc;

struct NoSecrets;
impl SecretStore for NoSecrets {
    fn read(&self, _: &str) -> AppResult<Option<String>> {
        Ok(None)
    }
    fn write(&self, _: &str, _: &str) -> AppResult<()> {
        Ok(())
    }
}
const ARCHIVE: &str = "4795349_49018171be025365913033bea897f2a9.zip";
const PAK: &str = "zzz_PBZAutoCollect_P.pak";

fn plan(engine: &FileEngine) -> install::InstallPreview {
    let id = uuid::Uuid::new_v4().to_string();
    let bucket = engine.paths.staging().join("imports").join(&id);
    fs::create_dir_all(&bucket).unwrap();
    let path = bucket.join(ARCHIVE);
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    zip.start_file(PAK, zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"test game content").unwrap();
    drop(zip.finish().unwrap());
    let entry = mod_library::ingest_import(
        engine,
        &ApprovedArtifact {
            artifact_id: &id,
            path: &path,
            filename: ARCHIVE,
            size: fs::metadata(&path).unwrap().len(),
            sha256: &crate::util::sha256_file(&path).unwrap(),
        },
        "Game",
        Some("g"),
    )
    .unwrap();
    install::plan_library_install(
        engine,
        &LibraryInstallRequest {
            game_id: "g".into(),
            library_entry_id: entry.id,
            replace_install_id: None,
            mode: LibraryInstallMode::ApprovedMappings {
                mod_type: None,
                source_kind: LibrarySourceKind::Archive,
                mappings: vec![ManualMapping {
                    source: PAK.into(),
                    destination: format!("Content/Paks/~mods/{PAK}"),
                }],
            },
        },
    )
    .unwrap()
}

#[tokio::test]
async fn install_commit_runs_organizer_without_an_agent_turn() {
    verify_automatic_organization(false).await;
}

#[tokio::test]
async fn startup_repairs_preexisting_hash_name_and_other_tag_without_reinstalling() {
    verify_automatic_organization(true).await;
}

async fn verify_automatic_organization(preexisting: bool) {
    let (_temp, engine, root) = crate::transaction::tests::setup();
    let settings = Settings::with_secret_store(engine.db.clone(), Arc::new(NoSecrets));
    crate::settings::set_mod_organization_locale_override(&engine.db, Some("zh-CN")).unwrap();
    let (sent, mut requests) = mpsc::unbounded_channel();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |body: Bytes| {
            let sent = sent.clone();
            async move {
                sent.send(serde_json::from_slice::<Value>(&body).unwrap())
                    .unwrap();
                let result =
                    json!({"displayName":"自动收集战利品","tags":["功能增强"]}).to_string();
                let frame =
                    json!({"choices":[{"delta":{"content":result},"finish_reason":"stop"}]});
                (
                    [("content-type", "text/event-stream")],
                    format!("data: {frame}\n\ndata: [DONE]\n\n"),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    settings
        .save(&AppSettings {
            base_url: format!("http://{}/v1", listener.local_addr().unwrap()),
            model: "organizer-fixture".into(),
            ..Default::default()
        })
        .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let preview = plan(&engine);
    let commit = || {
        engine.approve_plan(&preview.plan.id).unwrap();
        engine.apply_file_ops(&preview.plan.id).unwrap();
    };
    if preexisting {
        commit();
        // Recreate the v22 state reported in Game Details: a committed install
        // with the archive name, a generated catch-all tag, and no worker job.
        let db = engine.db.conn();
        db.execute(
            "DELETE FROM mod_organization_jobs WHERE install_id=?1",
            [&preview.install_id],
        )
        .unwrap();
        db.execute("INSERT INTO mod_tags(id,game_id,name,name_key,color) VALUES ('fallback-other','g','其他','其他','#666666')", []).unwrap();
        db.execute(
            "INSERT INTO install_mod_tags(install_id,tag_id) VALUES (?1,'fallback-other')",
            [&preview.install_id],
        )
        .unwrap();
    } else {
        jobs::initialize(&engine.db).unwrap();
        assert!(jobs::next(&engine.db).unwrap().is_none());
    }
    let (changed, mut changes) = mpsc::unbounded_channel();
    let worker = start(
        settings.clone(),
        engine.organization_wake.clone(),
        move |game, install| {
            let _ = changed.send((game.to_owned(), install.to_owned()));
        },
    );
    if !preexisting {
        commit();
    }
    let (game_id, install_id) = tokio::time::timeout(Duration::from_secs(15), changes.recv())
        .await
        .expect("worker should organize the committed install")
        .unwrap();
    assert_eq!(game_id, "g");
    assert_eq!(install_id, preview.install_id);
    let body = requests.recv().await.unwrap();
    assert!(body.get("tools").is_none());
    assert_eq!(
        body["messages"][0]["content"],
        skills::enabled_mod_organizer(&engine.db).unwrap().unwrap()
    );
    let facts = body["messages"][1]["content"].as_str().unwrap();
    assert!(facts.contains("zh-CN") && facts.contains(PAK));
    assert!(!facts.contains(_temp.path().file_name().unwrap().to_str().unwrap()));
    let record = install::get_install(&engine, &install_id).unwrap();
    assert_eq!(record.name, "自动收集战利品");
    let tags = list_mod_tags(&engine.db, &game_id).unwrap();
    assert_eq!(
        record.tag_ids,
        [tags
            .iter()
            .find(|tag| tag.name == "功能增强")
            .unwrap()
            .id
            .clone()]
    );
    assert_eq!(
        fs::read(root.join("Content/Paks/~mods").join(PAK)).unwrap(),
        b"test game content"
    );
    drop(worker);
    jobs::initialize(&engine.db).unwrap();
    assert!(jobs::next(&engine.db).unwrap().is_none());
    server.abort();
}

#[test]
fn accepts_only_a_single_organization_decision() {
    let suggestion =
        decode_reply("```json\n{\"displayName\":\"地图标记\",\"tags\":[\"地图\"]}\n```")
            .unwrap()
            .unwrap();
    assert_eq!(suggestion.display_name, "地图标记");
    assert!(decode_reply(r#"{"insufficientEvidence":true}"#)
        .unwrap()
        .is_none());
    for bad in [
        r#"{"insufficientEvidence":false}"#,
        r#"{"displayName":"Mod","tags":[],"installId":"wrong"}"#,
        "已整理",
    ] {
        assert!(decode_reply(bad).is_err(), "{bad}");
    }
}
