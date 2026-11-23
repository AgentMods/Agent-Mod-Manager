use super::*;

fn install(db: &Db, id: &str, name: &str, metadata: Value) {
    db.conn().execute("INSERT INTO installs(id,game_id,name,metadata_json,installed_at,updated_at) VALUES (?1,'g',?2,?3,'now','now')", params![id,name,metadata.to_string()]).unwrap();
}

fn suggestion(name: &str) -> OrganizationSuggestion {
    OrganizationSuggestion {
        display_name: name.into(),
        tags: vec![" Audio ".into(), "audio".into()],
    }
}

#[test]
fn backfills_active_legacy_and_rejects_non_committed_or_hidden() {
    let (_temp, engine, _) = crate::transaction::tests::setup();
    let db = &engine.db;
    for id in ["legacy", "hidden", "pending", "rolled", "trusted"] {
        install(db, id, "Sound Overhaul", json!({}));
    }
    db.conn().execute_batch("INSERT INTO removed_mod_items VALUES ('g','install','hidden');
      INSERT INTO file_transactions(id,kind,status,plan_json,created_at) VALUES ('p','install','approved','{}','now'),('r','install','rolled_back','{}','now');
      INSERT INTO install_transactions VALUES ('pending','p','install','now'),('rolled','r','install','now');
      INSERT INTO install_organizations VALUES ('trusted','A Curated Soundscape',0);
      INSERT INTO mod_tags VALUES ('curated','g','Ambience','ambience','#000000');
      INSERT INTO install_mod_tags VALUES ('trusted','curated');").unwrap();
    initialize(db).unwrap();
    initialize(db).unwrap();
    let job = next(db).unwrap().unwrap();
    assert_eq!(job.install_id, "legacy");
    insufficient(db, &job).unwrap();
    let reopened = Db::open(&engine.paths.db_file()).unwrap();
    initialize(&reopened).unwrap();
    assert!(next(&reopened).unwrap().is_none());
    assert_eq!(
        reopened
            .conn()
            .query_row("SELECT count(*) FROM mod_organization_jobs", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn finalize_is_atomic_and_retry_allows_next_job() {
    let (_temp, engine, _) = crate::transaction::tests::setup();
    let db = &engine.db;
    install(
        db,
        "new",
        "Texture Pack",
        json!({"organization":{"locale":"en"}}),
    );
    db.conn().execute_batch("INSERT INTO file_transactions(id,kind,status,plan_json,created_at) VALUES ('t','install','approved','{}','now'); INSERT INTO install_transactions VALUES ('new','t','install','now');").unwrap();
    {
        let mut conn = db.conn();
        let tx = conn.transaction().unwrap();
        assert!(!crate::mod_organization::finalize(&tx, "t").unwrap());
        tx.execute(
            "UPDATE file_transactions SET status='committed' WHERE id='t'",
            [],
        )
        .unwrap();
        assert!(crate::mod_organization::finalize(&tx, "t").unwrap());
        assert!(!crate::mod_organization::finalize(&tx, "t").unwrap());
        tx.rollback().unwrap();
    }
    assert!(next(db).unwrap().is_none());
    {
        let mut conn = db.conn();
        let tx = conn.transaction().unwrap();
        tx.execute(
            "UPDATE file_transactions SET status='committed' WHERE id='t'",
            [],
        )
        .unwrap();
        assert!(crate::mod_organization::finalize(&tx, "t").unwrap());
        tx.commit().unwrap();
    }
    install(db, "later", "Sound Overhaul", json!({}));
    initialize(db).unwrap();
    let job = next(db).unwrap().unwrap();
    assert_eq!(job.install_id, "new");
    retry(db, &job).unwrap();
    assert_eq!(next(db).unwrap().unwrap().install_id, "later");
    let delay: i64 = db
        .conn()
        .query_row(
            "SELECT ready_at-unixepoch() FROM mod_organization_jobs WHERE install_id='new'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!((1..=15).contains(&delay));
}

#[test]
fn projects_bound_receipts_and_validates_before_writing_preserving_manual_tags() {
    let (_temp, engine, _) = crate::transaction::tests::setup();
    let db = &engine.db;
    install(
        db,
        "mod",
        "28876144",
        json!({"artifactPath":"C:/Users/Private/28876144.zip","artifactSha256":"digest","libraryEntryId":"lib",
      "downloadOrigin":{"artifactId":"artifact","sourceId":"provider","projectId":"mod-id","versionId":"version"},"mission":"secret mission"}),
    );
    db.conn().execute_batch(r#"UPDATE installs SET install_root='C:/Install', files_json='[{"path":"C:/Install/assets/My Sound.ogg"},{"path":"C:/Private/Outside.txt"},{"path":"C:/Install/private/.env"}]' WHERE id='mod';
      INSERT INTO download_artifacts VALUES ('artifact','C:/private/download','My Sound.zip','{"sha256":"digest"}','provider','mod-id','version','My Sound','1','now');
      INSERT INTO mod_library_categories VALUES ('category','folder');
      INSERT INTO mod_library_entries(id,category_key,game_name,game_name_key,name,filename,archive_relative_path,payload_relative_path,size,sha256,source_id,version,integrity_verified,created_at,state)
      VALUES ('lib','category','Game','game','Curated Sound','Curated Sound.zip','one','two',1,'digest','web','1',1,'now','ready');"#).unwrap();
    initialize(db).unwrap();
    let job = next(db).unwrap().unwrap();
    assert_eq!(job.evidence["downloadReceipt"]["title"], "My Sound");
    assert_eq!(job.evidence["libraryEntry"]["title"], "Curated Sound");
    assert_eq!(job.evidence["fileName"], "Curated Sound.zip");
    assert_eq!(
        job.evidence["relativeFilePaths"],
        json!(["assets/My Sound.ogg"])
    );
    for secret in ["C:/", "Outside", ".env", "secret mission"] {
        assert!(!job.evidence.to_string().contains(secret));
    }
    for bad in [
        "",
        "28876144",
        "mod-28876144.7z",
        "d41d8cd98f00b204e9800998ecf8427e",
    ] {
        assert!(complete(db, &job, &suggestion(bad)).is_err());
    }
    assert!(complete(
        db,
        &job,
        &OrganizationSuggestion {
            display_name: "My Sound".into(),
            tags: vec!["".into()]
        }
    )
    .is_err());
    let manual = crate::mod_organization::save_mod_tag(db, "g", None, "Manual", "#010101").unwrap();
    crate::mod_organization::set_mod_tags(db, "g", "mod", &[manual.id]).unwrap();
    assert!(complete(db, &job, &suggestion("我的音效")).unwrap());
    assert!(!complete(db, &job, &suggestion("Different Name")).unwrap());
    assert_eq!(
        crate::mod_organization::list_mod_tags(db, "g")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn suppresses_unfounded_title_and_rechecks_skill_and_install_visibility() {
    let (_temp, engine, _) = crate::transaction::tests::setup();
    let db = &engine.db;
    install(
        db,
        "unclear",
        "mod-183794",
        json!({"artifactPath":"C:/downloads/mod-183794.zip"}),
    );
    initialize(db).unwrap();
    let job = next(db).unwrap().unwrap();
    assert!(complete(db, &job, &suggestion("Invented Title")).is_err());
    db.conn().execute_batch("CREATE TABLE IF NOT EXISTS system_skill_preferences(name TEXT PRIMARY KEY,enabled INTEGER NOT NULL); INSERT INTO system_skill_preferences VALUES ('system-mod-organize',0);").unwrap();
    assert!(!complete(db, &job, &suggestion("Invented Title")).unwrap());
    insufficient(db, &job).unwrap();
    assert!(next(db).unwrap().is_some());
    db.conn().execute_batch("UPDATE system_skill_preferences SET enabled=1; UPDATE installs SET status='uninstalled' WHERE id='unclear';").unwrap();
    assert!(!complete(db, &job, &suggestion("Invented Title")).unwrap());
    assert!(next(db).unwrap().is_none());
}

#[test]
fn translates_distinctive_archive_and_manifest_stems_without_a_staged_suggestion() {
    let (_temp, engine, _) = crate::transaction::tests::setup();
    let db = &engine.db;
    install(
        db,
        "collect",
        "904221",
        json!({"artifactPath":"C:/Downloads/zzz_PBZAutoCollect_P.pak"}),
    );
    install(db, "map", "904222", json!({}));
    install(
        db,
        "generic",
        "904223",
        json!({"artifactPath":"C:/Downloads/904223.zip"}),
    );
    db.conn().execute_batch(r#"
       UPDATE installs SET status='installed' WHERE id='collect';
       UPDATE installs SET files_json='[{"path":"Paks/~mods/zzz_PBZMapMarkers_P.pak"}]' WHERE id='map';
       UPDATE installs SET files_json='[{"path":"data/config/manifest/main.js"}]' WHERE id='generic';
    "#).unwrap();
    initialize(db).unwrap();
    for (id, title) in [("collect", "自动收集战利品"), ("map", "地图收集标记")] {
        let job = next(db).unwrap().unwrap();
        assert_eq!(job.install_id, id);
        assert!(complete(db, &job, &suggestion(title)).unwrap());
    }
    let job = next(db).unwrap().unwrap();
    assert_eq!(job.install_id, "generic");
    assert!(complete(db, &job, &suggestion("地图收集标记")).is_err());
}

#[test]
fn disabled_install_preserves_manual_tags_and_no_evidence_install_skips_other() {
    let (_temp, engine, _) = crate::transaction::tests::setup();
    let db = &engine.db;
    install(db, "prior", "Better Sound", json!({}));
    install(
        db,
        "replacement",
        "Better Sound",
        json!({"organization":{"locale":"en","displayName":"New Sound","tags":["Automatic"]}}),
    );
    install(
        db,
        "unclear",
        "835711",
        json!({"organization":{"locale":"en"}}),
    );
    let manual = crate::mod_organization::save_mod_tag(db, "g", None, "Manual", "#010101").unwrap();
    crate::mod_organization::set_mod_tags(db, "g", "prior", std::slice::from_ref(&manual.id))
        .unwrap();
    let reusable =
        crate::mod_organization::save_mod_tag(db, "g", None, "Reusable", "#020202").unwrap();
    db.conn().execute_batch("CREATE TABLE IF NOT EXISTS system_skill_preferences(name TEXT PRIMARY KEY,enabled INTEGER NOT NULL);
        INSERT INTO system_skill_preferences VALUES ('system-mod-organize',0);
        INSERT INTO file_transactions(id,kind,status,plan_json,created_at) VALUES ('a','install','committed','{}','now'),('b','install','committed','{}','now');
        INSERT INTO install_transactions VALUES ('replacement','a','install','now'),('unclear','b','install','now');
        INSERT INTO install_replacements VALUES ('a','prior','replacement');").unwrap();
    {
        let mut conn = db.conn();
        let tx = conn.transaction().unwrap();
        assert!(crate::mod_organization::finalize(&tx, "a").unwrap());
        tx.execute("UPDATE system_skill_preferences SET enabled=1", [])
            .unwrap();
        assert!(crate::mod_organization::finalize(&tx, "b").unwrap());
        tx.commit().unwrap();
    }
    let job = next(db).unwrap().unwrap();
    assert_eq!(job.install_id, "replacement");
    assert_eq!(job.evidence["currentDisplayName"], Value::Null);
    assert_eq!(job.evidence["assignedTags"], json!([manual.name]));
    assert_eq!(
        job.evidence["existingTags"],
        json!([manual.name, reusable.name])
    );
}
