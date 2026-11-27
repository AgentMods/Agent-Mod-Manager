use crate::{
    agent::{
        install_tools,
        policy::{Effect, PermissionMode},
        tools::{ToolContext, ToolRegistry},
    },
    db::Db,
    error::AppResult,
    sources::resolutions::ResolutionScope,
    transaction::{self, FileEngine},
};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

struct Fixture {
    _dir: tempfile::TempDir,
    engine: FileEngine,
    registry: ToolRegistry,
    game: PathBuf,
    external: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let (dir, engine, game) = transaction::tests::setup();
        let external = dir.path().join("files");
        fs::create_dir(&external).unwrap();
        let registry = ToolRegistry::builtin().unwrap();
        install_tools::register(&registry).unwrap();
        Self {
            _dir: dir,
            engine,
            registry,
            game,
            external,
        }
    }
    fn context(&self, resolution_scope: ResolutionScope) -> ToolContext {
        ToolContext {
            engine: self.engine.clone(),
            cancel: CancellationToken::new(),
            progress: None,
            resolution_scope,
        }
    }
    fn agent(&self) -> ToolContext {
        self.context(ResolutionScope::Agent(
            crate::history::History::new(self.engine.db.clone())
                .create_conversation("Local files", None)
                .unwrap()
                .id,
        ))
    }
    fn restricted(&self) -> [ToolContext; 2] {
        [
            self.context(ResolutionScope::Desktop),
            self.context(ResolutionScope::Mcp("remote-session".into())),
        ]
    }
    async fn call(&self, ctx: &ToolContext, name: &str, args: Value) -> AppResult<Value> {
        let tool = self.registry.get(name)?;
        let prepared = tool.prepare(ctx, &args).await?;
        tool.execute(ctx, args, prepared).await
    }
    async fn plan(&self, ctx: &ToolContext, ops: Value) -> AppResult<String> {
        let result = self
            .call(
                ctx,
                "plan_file_ops",
                json!({"operations":ops,"summary":"fixture"}),
            )
            .await?;
        let id = result["id"].as_str().unwrap();
        let stored = ctx.file_engine().get_plan(id).unwrap();
        assert_eq!(result, json!(stored.into_user_paths()));
        Ok(id.into())
    }
    async fn apply_after_review(&self, id: &str, effect: Effect) {
        let ctx = self.agent();
        let tool = self.registry.get("apply_file_ops").unwrap();
        let args = json!({"plan_id":id});
        let prepared = tool.prepare(&ctx, &args).await.unwrap();
        assert_eq!(prepared.effect, effect);
        assert!(PermissionMode::AutoApprove.needs_approval(effect));
        assert!(!PermissionMode::FullTrust.needs_approval(effect));
        let stored = ctx.file_engine().get_plan(id).unwrap();
        assert_eq!(json!(prepared.plan.as_ref().unwrap()), json!(stored));
        let displayed = json!(stored.into_user_paths());
        assert_eq!(prepared.preview, displayed);
        assert_eq!(tool.execute(&ctx, args, prepared).await.unwrap(), displayed);
    }
    fn reopen(&self) -> FileEngine {
        let paths = self.engine.paths.as_ref().clone();
        FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths)
    }
    fn status(&self, id: &str) -> String {
        self.engine
            .db
            .conn()
            .query_row(
                "SELECT status FROM file_transactions WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
    }
}
fn write(dst: &Path, content: &str) -> Value {
    json!({"op":"write_text","dst":dst,"content":content})
}

#[tokio::test]
async fn agent_can_inspect_game_parent_and_sibling_files_without_registering_them() {
    let f = Fixture::new();
    let parent = f.game.parent().unwrap();
    let note = parent.join("appmanifest_123.acf");
    fs::write(&note, "Steam fixture").unwrap();
    let archive = f.external.join("mod.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    zip.start_file("entry.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"fixture").unwrap();
    zip.finish().unwrap();
    let agent = f.agent();
    let list = f
        .call(&agent, "read_dir", json!({"path":parent}))
        .await
        .unwrap();
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["name"] == "appmanifest_123.acf"));
    let text = f
        .call(&agent, "read_file", json!({"path":note}))
        .await
        .unwrap();
    assert_eq!(text["lines"][0]["text"], "Steam fixture");
    let listing = f
        .call(&agent, "extract_preview", json!({"path":archive}))
        .await
        .unwrap();
    assert_eq!(listing["entries"][0]["path"], "entry.txt");
    for ctx in f.restricted() {
        for (name, path) in [
            ("read_dir", parent),
            ("read_file", note.as_path()),
            ("extract_preview", archive.as_path()),
        ] {
            assert!(f.call(&ctx, name, json!({"path":path})).await.is_err());
        }
    }
}

#[tokio::test]
async fn extract_preview_accepts_rar5_in_downloads_incoming() {
    let fixture = Fixture::new();
    let incoming = fixture.engine.paths.downloads().join("incoming");
    fs::create_dir_all(&incoming).unwrap();
    let archive = incoming.join("sample.rar");
    fs::write(
        &archive,
        include_bytes!("../../tests/fixtures/archive-rar/rar5-mixed.rar"),
    )
    .unwrap();
    let listing = fixture
        .call(
            &fixture.agent(),
            "extract_preview",
            json!({"path": archive}),
        )
        .await
        .unwrap();
    assert_eq!(listing["format"], "rar");
    assert_eq!(listing["entries"].as_array().unwrap().len(), 2);
    assert_eq!(listing["total_size"], 307);
    assert_eq!(fs::read_dir(incoming).unwrap().count(), 1);
}

#[tokio::test]
async fn agent_read_file_lists_playsets_backup_and_can_read_a_returned_path() {
    let f = Fixture::new();
    let backup = f.external.join("playsets_backup");
    let child_dir = backup.join("profiles");
    fs::create_dir_all(&child_dir).unwrap();
    fs::write(child_dir.join("nested.json"), "{}").unwrap();
    let note = backup.join("playset.json");
    fs::write(&note, "{\n}\n").unwrap();

    let tool = f.registry.get("read_file").unwrap();
    assert!(tool.definition().description.contains("直接子项"));
    assert!(!tool.inbound_definition().description.contains("直接子项"));
    let agent = f.agent();
    let args = json!({"path":backup});
    let prepared = tool.prepare(&agent, &args).await.unwrap();
    assert_eq!(prepared.effect, Effect::Read);
    let dir = tool.execute(&agent, args.clone(), prepared).await.unwrap();
    let display = |path: &Path| crate::paths::display_path(&path.canonicalize().unwrap());
    assert_eq!(
        dir,
        json!({
            "kind":"directory",
            "path":display(&backup),
            "entries":[
                {"name":"playset.json","path":display(&note),"is_dir":false},
                {"name":"profiles","path":display(&child_dir),"is_dir":true},
            ],
            "truncated":false,
        })
    );
    let text = f
        .call(
            &agent,
            "read_file",
            json!({"path":dir["entries"][0]["path"]}),
        )
        .await
        .unwrap();
    assert_eq!(
        text,
        json!({
            "lines":[{"line":1,"text":"{"},{"line":2,"text":"}"}], "truncated":false
        })
    );
    for context in f.restricted() {
        assert!(matches!(
            f.call(&context, "read_file", args.clone()).await,
            Err(crate::error::AppError::PathEscape(_))
        ));
        assert!(f
            .call(&context, "read_file", json!({"path":f.game}))
            .await
            .is_err());
    }
    let binary = backup.join("binary.dat");
    fs::write(&binary, b"a\0b").unwrap();
    assert!(matches!(
        f.call(&agent, "read_file", json!({"path":binary})).await,
        Err(crate::error::AppError::Invalid(_))
    ));
    assert!(matches!(
        f.call(&agent, "read_file", json!({"path":backup.join("missing")}))
            .await,
        Err(crate::error::AppError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound
    ));
}

#[tokio::test]
async fn agent_native_directory_tools_page_but_restricted_tools_keep_the_old_contract() {
    let f = Fixture::new();
    let ctx = f.agent();
    let dir = f.game.join("ordered");
    fs::create_dir(&dir).unwrap();
    for name in ["e.txt", "b.txt", "a.txt", "d.txt"] {
        fs::write(dir.join(name), name).unwrap();
    }
    fs::create_dir(dir.join("c-dir")).unwrap();

    for name in ["read_dir", "read_file"] {
        let tool = f.registry.get(name).unwrap();
        assert_eq!(
            tool.definition().parameters["properties"]["offset"]["minimum"],
            0
        );
        assert_eq!(
            tool.inbound_definition().parameters,
            json!({
                "type":"object", "properties":{"path":{"type":"string"}},
                "required":["path"], "additionalProperties":false
            })
        );
        let mut offset = json!(0);
        let mut names = Vec::new();
        loop {
            let page = f
                .call(&ctx, name, json!({"path":dir,"offset":offset,"limit":2}))
                .await
                .unwrap();
            let entries = page["entries"].as_array().unwrap();
            assert!(entries.len() <= 2);
            for entry in entries {
                let entry_name = entry["name"].as_str().unwrap();
                names.push(entry_name.to_owned());
                assert_eq!(entry["is_dir"], entry_name == "c-dir");
                if name == "read_dir" && entry_name != "c-dir" {
                    assert_eq!(entry["size"], entry_name.len());
                    assert!(!entry["modified"].as_str().unwrap().is_empty());
                }
            }
            offset = page["next_offset"].clone();
            assert_eq!(page["truncated"], !offset.is_null());
            if offset.is_null() {
                break;
            }
            assert_eq!(offset, names.len());
        }
        assert_eq!(names, ["a.txt", "b.txt", "c-dir", "d.txt", "e.txt"]);
        let past_end = f
            .call(&ctx, name, json!({"path":dir,"offset":999}))
            .await
            .unwrap();
        assert_eq!(past_end["entries"], json!([]));
        assert_eq!(past_end["truncated"], false);
        for args in [
            json!({"path":dir,"offset":-1}),
            json!({"path":dir,"limit":0}),
            json!({"path":dir,"limit":2001}),
        ] {
            assert!(matches!(
                f.call(&ctx, name, args).await,
                Err(crate::error::AppError::Invalid(_))
            ));
        }
    }

    let file = dir.join("a.txt");
    assert!(f
        .call(&ctx, "read_dir", json!({"path":file,"offset":0}))
        .await
        .is_err());
    assert!(f
        .call(
            &ctx,
            "read_dir",
            json!({"path":dir.join("missing"),"offset":99})
        )
        .await
        .is_err());
    assert!(f
        .call(&ctx, "read_file", json!({"path":file,"offset":0}))
        .await
        .is_err());
    assert!(f
        .call(&ctx, "read_file", json!({"path":dir,"byte_offset":0}))
        .await
        .is_err());
    for restricted in f.restricted() {
        let listing = f
            .call(&restricted, "read_dir", json!({"path":dir}))
            .await
            .unwrap();
        assert_eq!(listing.as_array().unwrap().len(), 5);
        let text = f
            .call(&restricted, "read_file", json!({"path":file}))
            .await
            .unwrap();
        assert_eq!(
            text,
            json!({"lines":[{"line":1,"text":"a.txt"}],"truncated":false})
        );
        assert!(f
            .call(&restricted, "read_file", json!({"path":dir}))
            .await
            .is_err());
        // Even if a client supplies an undeclared field, it cannot select local paging or scope.
        assert_eq!(
            f.call(&restricted, "read_dir", json!({"path":dir,"limit":1}))
                .await
                .unwrap(),
            listing
        );
        assert_eq!(
            f.call(
                &restricted,
                "read_file",
                json!({"path":file,"byte_offset":2})
            )
            .await
            .unwrap(),
            text
        );
    }
}

#[tokio::test]
async fn agent_read_file_can_resume_beyond_both_previous_total_limits() {
    let f = Fixture::new();
    let ctx = f.agent();
    let file = f.external.join("giant.txt");
    let lines = "x\n".repeat(2001);
    let long = format!("{}🌱{}", "x".repeat(256 * 1024 - 2), "z".repeat(17));
    fs::write(&file, format!("{lines}{long}\r\nlast")).unwrap();

    let first = f
        .call(&ctx, "read_file", json!({"path":file}))
        .await
        .unwrap();
    assert_eq!(first["lines"].as_array().unwrap().len(), 2000);
    assert_eq!(first["next_byte_offset"], 4000);
    let second = f
        .call(
            &ctx,
            "read_file",
            json!({"path":file,"byte_offset":first["next_byte_offset"],"max_lines":1}),
        )
        .await
        .unwrap();
    assert_eq!(second["lines"], json!([{"line":2001,"text":"x"}]));
    let head = f
        .call(&ctx, "read_file", json!({"path":file,"start_line":2002}))
        .await
        .unwrap();
    assert_eq!(head["byte_offset"], second["next_byte_offset"]);
    assert_eq!(head["ends_mid_line"], true);
    assert_eq!(head["lines"][0]["line"], 2002);
    let tail = f
        .call(
            &ctx,
            "read_file",
            json!({"path":file,"byte_offset":head["next_byte_offset"]}),
        )
        .await
        .unwrap();
    assert_eq!(tail["starts_mid_line"], true);
    assert_eq!(tail["lines"][0]["line"], 2002);
    assert_eq!(
        format!(
            "{}{}",
            head["lines"][0]["text"].as_str().unwrap(),
            tail["lines"][0]["text"].as_str().unwrap()
        ),
        long
    );
    assert_eq!(tail["lines"][1], json!({"line":2003,"text":"last"}));
    assert_eq!(tail["truncated"], false);
    assert!(tail["next_byte_offset"].is_null());

    let inside_scalar = head["next_byte_offset"].as_u64().unwrap() + 1;
    let inside_crlf = lines.len() + long.len() + 1;
    for paging in [
        json!({"byte_offset":inside_scalar}),
        json!({"byte_offset":inside_crlf}),
        json!({"byte_offset":-1}),
        json!({"byte_offset":0,"start_line":1}),
        json!({"start_line":0}),
        json!({"max_lines":2001}),
        json!({"max_bytes":3}),
    ] {
        let mut args = paging;
        args["path"] = json!(file);
        assert!(matches!(
            f.call(&ctx, "read_file", args).await,
            Err(crate::error::AppError::Invalid(_))
        ));
    }
    for args in [
        json!({"path":file,"start_line":u64::MAX}),
        json!({"path":file,"byte_offset":u64::MAX}),
        json!({"path":file,"byte_offset":tail["end_byte_offset"]}),
    ] {
        let page = f.call(&ctx, "read_file", args).await.unwrap();
        assert_eq!(page["lines"], json!([]));
        assert_eq!(page["truncated"], false);
        assert!(page["next_byte_offset"].is_null());
    }
}

#[tokio::test]
async fn archive_sample_pages_keep_global_metadata_and_validate_unreturned_entries() {
    let f = Fixture::new();
    let ctx = f.agent();
    let archive = f.external.join("sample.zip");
    let make_zip = |second: &str| {
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        for name in ["release/readme.txt", second] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"fixture").unwrap();
        }
        zip.finish().unwrap();
    };
    make_zip("release/Mods/Sample/manifest.json");
    let first = f
        .call(&ctx, "extract_preview", json!({"path":archive, "limit":1}))
        .await
        .unwrap();
    assert_eq!(first["entries"].as_array().unwrap().len(), 1);
    assert_eq!(first["entries"][0]["path"], "release/readme.txt");
    assert_eq!(first["stripped_root"], "release");
    assert_eq!(first["total_entries"], 2);
    assert_eq!(first["next_offset"], 1);
    let second = f
        .call(
            &ctx,
            "extract_preview",
            json!({"path":archive, "offset":first["next_offset"], "limit":1}),
        )
        .await
        .unwrap();
    assert_eq!(
        second["entries"][0]["path"],
        "release/Mods/Sample/manifest.json"
    );
    assert_eq!(second["next_offset"], Value::Null);
    assert_eq!(second["total_size"], first["total_size"]);
    for limit in [0, 201] {
        assert!(f
            .call(
                &ctx,
                "extract_preview",
                json!({"path":archive, "limit":limit})
            )
            .await
            .is_err());
    }
    make_zip("../unsafe.txt");
    assert!(f
        .call(&ctx, "extract_preview", json!({"path":archive, "limit":1}))
        .await
        .is_err());
}

#[tokio::test]
async fn extract_preview_directory_error_is_localized_after_path_access_check() {
    let f = Fixture::new();
    let ctx = f.agent();
    let directory = f.external.join("game-files");
    fs::create_dir(&directory).unwrap();

    let error = f
        .call(
            &ctx,
            "extract_preview",
            json!({"path":directory,"legacy_extension":true}),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, crate::error::AppError::Invalid(ref detail)
        if detail == "此路径是目录，请使用 read_dir 列出目录内容。"));

    let [_, mcp] = f.restricted();
    let error = f
        .call(&mcp, "extract_preview", json!({"path":directory}))
        .await
        .unwrap_err();
    assert!(matches!(error, crate::error::AppError::PathEscape(_)));
}

#[tokio::test]
async fn mixed_external_plan_is_reviewed_and_cannot_be_claimed_by_mcp() {
    let f = Fixture::new();
    let src = f.external.join("source.txt");
    let moved = f.external.join("move.txt");
    fs::write(&src, "copy").unwrap();
    fs::write(&moved, "move").unwrap();
    let copy_dst = f.external.join("new/nested/copy.txt");
    let move_dst = f.game.join("move.txt");
    let dir = f.external.join("new/empty");
    let ops = json!([{"op":"copy","src":src,"dst":copy_dst},{"op":"move","src":moved,"dst":move_dst},{"op":"mkdir","dst":dir}]);
    let id = f.plan(&f.agent(), ops.clone()).await.unwrap();
    for ctx in f.restricted() {
        assert!(f.plan(&ctx, ops.clone()).await.is_err());
        assert!(f
            .call(&ctx, "apply_file_ops", json!({"plan_id":id}))
            .await
            .is_err());
        assert_eq!(f.status(&id), "planned");
    }
    assert!(!copy_dst.exists() && !move_dst.exists() && !dir.exists() && moved.exists());
    f.apply_after_review(&id, Effect::ExternalWrite).await;
    assert_eq!(fs::read_to_string(copy_dst).unwrap(), "copy");
    assert_eq!(fs::read_to_string(move_dst).unwrap(), "move");
    assert!(dir.is_dir() && !moved.exists());
}

#[tokio::test]
async fn external_utf8_create_and_overwrite_only_write_after_approval_and_can_roll_back() {
    let f = Fixture::new();
    let path = f.external.join("notes.txt");
    let before = "初始文本\n🛠";
    let after = "更新文本\n🌱";
    let id = f
        .plan(&f.agent(), json!([write(&path, before)]))
        .await
        .unwrap();
    assert!(!path.exists());
    f.apply_after_review(&id, Effect::ExternalWrite).await;
    let id = f
        .plan(&f.agent(), json!([write(&path, after)]))
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    f.apply_after_review(&id, Effect::ExternalWrite).await;
    let restarted = f.reopen();
    assert!(restarted.rollback_transaction(&id).is_err());
    for ctx in f.restricted() {
        assert!(f
            .call(&ctx, "rollback_transaction", json!({"transaction_id":id}))
            .await
            .is_err());
    }
    assert_eq!(f.status(&id), "committed");
    assert_eq!(fs::read_to_string(&path).unwrap(), after);
    restarted
        .for_local_agent()
        .rollback_transaction(&id)
        .unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), before);
}

#[tokio::test]
async fn startup_recovers_external_overwrite_from_its_journal() {
    let f = Fixture::new();
    let dst = f.external.join("existing.txt");
    fs::write(&dst, "original").unwrap();
    let id = f
        .plan(&f.agent(), json!([write(&dst, "replacement")]))
        .await
        .unwrap();
    f.apply_after_review(&id, Effect::ExternalWrite).await;
    assert_eq!(fs::read_to_string(&dst).unwrap(), "replacement");
    // Simulate process loss after all filesystem mutations but before the final status update.
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE file_transactions SET status='applying' WHERE id=?1",
            [&id],
        )
        .unwrap();
    assert_eq!(
        f.reopen().recover_interrupted().unwrap().as_slice(),
        std::slice::from_ref(&id)
    );
    assert_eq!(f.status(&id), "rolled_back");
    assert_eq!(fs::read_to_string(dst).unwrap(), "original");
}

#[tokio::test]
async fn write_text_is_desktop_agent_only_even_in_game_and_accepts_more_than_one_mib_in_aggregate()
{
    let f = Fixture::new();
    let dst = f.game.join("settings.txt");
    let ops = json!([write(&dst, "fixture")]);
    for ctx in f.restricted() {
        assert!(f.plan(&ctx, ops.clone()).await.is_err());
    }
    assert!(f
        .engine
        .plan_file_ops(
            serde_json::from_value(ops.clone()).unwrap(),
            "restricted".into()
        )
        .is_err());
    let id = f.plan(&f.agent(), ops).await.unwrap();
    assert!(!dst.exists());
    f.apply_after_review(&id, Effect::ManagerWrite).await;
    assert_eq!(fs::read_to_string(&dst).unwrap(), "fixture");
    let large = "x".repeat(1024 * 512 + 1);
    let other = f.game.join("other.txt");
    let id = f
        .plan(
            &f.agent(),
            json!([write(&dst, &large), write(&other, &large)]),
        )
        .await
        .unwrap();
    assert!(!other.exists());
    f.apply_after_review(&id, Effect::ManagerWrite).await;
    assert_eq!(fs::read_to_string(dst).unwrap(), large);
    assert_eq!(fs::read_to_string(other).unwrap(), large);
}

#[tokio::test]
async fn agent_can_write_and_read_a_custom_storage_folder_with_manager_approval() {
    let f = Fixture::new();
    let agent = f.agent();
    let storage = f
        .call(&agent, "get_storage_directory", json!({}))
        .await
        .unwrap();
    let dst = Path::new(storage["path"].as_str().unwrap()).join("custom/config.ini");
    let id = f
        .plan(&agent, json!([write(&dst, "enabled=true")]))
        .await
        .unwrap();
    assert!(!dst.exists());
    f.apply_after_review(&id, Effect::ManagerWrite).await;
    let read = f
        .call(&agent, "read_file", json!({"path":dst}))
        .await
        .unwrap();
    assert_eq!(read["lines"][0]["text"], "enabled=true");
}

#[tokio::test]
async fn local_file_access_still_protects_env_and_manager_credentials() {
    let f = Fixture::new();
    let private = f.engine.paths.root.join("private");
    fs::create_dir_all(&private).unwrap();
    let agent = f.agent();
    assert!(f
        .call(&agent, "read_dir", json!({"path":private}))
        .await
        .is_err());
    let env_directory = f.external.join(".env.backup");
    fs::create_dir(&env_directory).unwrap();
    for directory in [&private, &env_directory] {
        assert!(matches!(
            f.call(&agent, "read_file", json!({"path":directory})).await,
            Err(crate::error::AppError::Denied(_))
        ));
    }
    for path in [
        private.join("credentials.json"),
        f.external.join(".env.local"),
    ] {
        fs::write(&path, "fixture secret").unwrap();
        assert!(f
            .call(&agent, "read_file", json!({"path":path}))
            .await
            .is_err());
        assert!(f
            .plan(&agent, json!([write(&path, "overwrite")]))
            .await
            .is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "fixture secret");
    }
    assert!(f
        .plan(
            &agent,
            json!([{"op":"move","src":f.external,"dst":f.game.join("newdir")}])
        )
        .await
        .is_err());
}

#[tokio::test]
async fn archive_file_ops_are_reviewed_in_registered_roots_and_unavailable_to_inbound_mcp() {
    let f = Fixture::new();
    let tool = f.registry.get("plan_file_ops").unwrap();
    let local = tool.definition();
    let inbound = tool.inbound_definition();
    for new_op in ["compress", "extract"] {
        let allowed = |definition: &super::tools::ToolDefinition| {
            definition.parameters["properties"]["operations"]["items"]["properties"]["op"]["enum"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == new_op)
        };
        assert!(allowed(&local));
        assert!(!allowed(&inbound));
    }

    let src = f.game.join("folder");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("readme.txt"), "test payload").unwrap();
    let archive = f.game.join("result.zip");
    let dst = f.game.join("unpacked");
    let agent = f.agent();
    for (op, src, dst) in [("compress", &src, &archive), ("extract", &archive, &dst)] {
        let operations = json!([{"op":op,"src":src,"dst":dst}]);
        for ctx in f.restricted() {
            assert!(f.plan(&ctx, operations.clone()).await.is_err());
        }
        let id = f.plan(&agent, operations).await.unwrap();
        assert!(!dst.exists());
        let apply_tool = f.registry.get("apply_file_ops").unwrap();
        for ctx in f.restricted() {
            assert!(apply_tool
                .prepare(&ctx, &json!({"plan_id":id}))
                .await
                .is_err());
        }
        assert_eq!(f.status(&id), "planned");
        f.apply_after_review(&id, Effect::ManagerWrite).await;
    }
    assert_eq!(
        fs::read_to_string(dst.join("folder/readme.txt")).unwrap(),
        "test payload"
    );
    assert!(src.is_dir() && archive.is_file());
}
