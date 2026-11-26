//! Disposable fixtures. Synthetic RPF7-shaped files exercise native authority,
//! generation accounting and real FileEngine I/O, NOT the external RPF editor.
use super::*;
use crate::{db::Db, storage::AppPaths};
use std::fs;
fn container(tag: &str) -> Vec<u8> {
    let mut b = b"7FPR".to_vec();
    b.extend(1u32.to_le_bytes());
    b.extend(1u32.to_le_bytes());
    b.extend(b"OPEN");
    b.extend([0; 17]);
    b.extend(tag.as_bytes());
    b
}
struct World {
    _temp: tempfile::TempDir,
    e: FileEngine,
    g: games::Game,
    c: CancellationToken,
}
impl World {
    fn new() -> Self {
        Self::for_edition(Edition::Legacy)
    }
    fn for_edition(edition: Edition) -> Self {
        let (slug, manifest) = match edition {
            Edition::Legacy => (
                "gta5-legacy",
                include_str!("../../../../packages/gta-adapters/gta5-legacy/adapter.json"),
            ),
            Edition::Enhanced => (
                "gta5-enhanced",
                include_str!("../../../../packages/gta-adapters/gta5-enhanced/adapter.json"),
            ),
        };
        let temp = tempfile::tempdir().unwrap();
        let p = AppPaths::new(temp.path().join("data")).unwrap();
        let e = FileEngine::new(Db::open(&p.db_file()).unwrap(), p);
        let root = temp.path().join("gta");
        fs::create_dir_all(root.join("update")).unwrap();
        fs::write(root.join(edition.marker()), edition.marker().as_bytes()).unwrap();
        fs::write(root.join("update/update.rpf"), container("vanilla")).unwrap();
        e.db.conn().execute("INSERT INTO plugins(slug,name,game_name,version,source,install_path,adapter_json,created_at,updated_at) VALUES(?1,'test','Grand Theft Auto V',json_extract(?2,'$.version'),'local','test',?2,'now','now')",params![slug, manifest]).unwrap();
        let g = games::register_game(&e.db, &root, Some(slug)).unwrap();
        assert_eq!(g.platform, "windows");
        Self {
            _temp: temp,
            e,
            g,
            c: CancellationToken::new(),
        }
    }
    fn import(&self, filename: &str, src: &[u8]) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let d = self.e.paths.staging().join("imports").join(&id);
        fs::create_dir_all(&d).unwrap();
        let p = d.join(filename);
        fs::write(&p, src).unwrap();
        mod_library::ingest_import(
            &self.e,
            &mod_library::ApprovedArtifact {
                artifact_id: &id,
                path: &p,
                filename,
                size: src.len() as u64,
                sha256: &hex::encode(Sha256::digest(src)),
            },
            &self.g.name,
            Some(&self.g.id),
        )
        .unwrap()
        .id
    }
    fn prep(&self, a: &str, k: &str, o: Value) -> AppResult<String> {
        Ok(prepare_job(
            &self.e,
            &self.g.id,
            Some(a),
            None,
            Some(k),
            Some(&o),
            &self.c,
        )?
        .job_id)
    }
    fn remove(&self, id: &str) -> AppResult<String> {
        Ok(prepare_job(&self.e, &self.g.id, None, Some(id), None, None, &self.c)?.job_id)
    }
    fn publish(&self, id: &str) {
        let j = frozen(&self.e, id).unwrap();
        let dir = jobs(&self.e).unwrap().join(id);
        fs::create_dir(dir.join("output")).unwrap();
        let mut outputs = vec![];
        for t in j.input.targets {
            let tags = t
                .contributions
                .iter()
                .map(|co| match co {
                    Contribution::Dlc { pack_name, .. } => pack_name.as_str(),
                    _ => co.kind(),
                })
                .collect::<Vec<_>>()
                .join("+");
            let bytes = if t.id == "menyoo" {
                format!("<AddedVehicleModels><!--{tags}--></AddedVehicleModels>").into_bytes()
            } else {
                container(&tags)
            };
            fs::write(dir.join("output").join(output_name(&t.id).unwrap()), &bytes).unwrap();
            outputs.push(
                json!({"id":t.id,"size":bytes.len(),"sha256":hex::encode(Sha256::digest(bytes))}),
            );
        }
        fs::write(
            dir.join("result.json"),
            json!({"schemaVersion":1,"jobId":id,"status":"ok","outputs":outputs}).to_string(),
        )
        .unwrap();
    }
    fn commit(&self, p: &FilePlan) {
        self.e.approve_plan(&p.id).unwrap();
        self.e.apply_file_ops(&p.id).unwrap();
    }
    fn apply_new(&self, a: &str, k: &str, o: Value) -> InstallPreview {
        let id = self.prep(a, k, o).unwrap();
        self.publish(&id);
        let preview = plan_install(&self.e, &self.g.id, a, &id, None, &self.c).unwrap();
        self.commit(&preview.plan);
        preview
    }
    fn delete(&self, id: &str) -> FilePlan {
        let job = self.remove(id).unwrap();
        self.publish(&job);
        let p = plan_uninstall(&self.e, &self.g.id, id, &job, &self.c).unwrap();
        self.commit(&p);
        p
    }
    fn p(&self, key: &str) -> PathBuf {
        destination(Path::new(&self.g.root_path), key).unwrap()
    }
}
#[test]
fn gta_rpf_dlc_removes_non_latest_contribution_and_rolls_back_only_the_head() {
    let w = World::new();
    let a = w.import("dlc.rpf", &container("alpha"));
    let b = w.import("dlc.rpf", &container("beta"));
    let first = w.apply_new(&a, "dlc", json!({"packName":"alpha"}));
    let second = w.apply_new(&b, "dlc", json!({"packName":"beta"}));
    assert_eq!(fs::read(w.p("update")).unwrap(), container("alpha+beta"));
    assert!(w.e.rollback_transaction(&first.plan.id).is_err());
    assert!(w.prep(&a, "dlc", json!({"packName":"other"})).is_err());
    let found = inspect(
        &w.e,
        &super::super::get_install(&w.e, &first.install_id).unwrap(),
    )
    .unwrap();
    assert!(found.can_uninstall);
    assert!(found
        .files
        .iter()
        .any(|f| f.other_install_ids.contains(&second.install_id)));
    let current = active_files(&w.e, &w.g.id, &first.install_id).unwrap();
    let shared = current.iter().find(|f| f.path == w.p("update")).unwrap();
    assert_eq!(
        shared.sha256,
        hex::encode(Sha256::digest(container("alpha+beta")))
    );
    fs::write(w.p("dlc:beta"), container("manual change")).unwrap();
    let blocked = super::super::inspect_install(&w.e, &first.install_id).unwrap();
    assert!(!blocked.can_uninstall);
    assert!(blocked
        .reconciliation
        .blocking_install_ids
        .contains(&second.install_id));
    fs::write(w.p("dlc:beta"), container("beta")).unwrap();
    let removed = w.delete(&first.install_id);
    assert!(active_files(&w.e, &w.g.id, &first.install_id)
        .unwrap()
        .is_empty());
    assert!(!w.p("dlc:alpha").exists());
    assert_eq!(fs::read(w.p("update")).unwrap(), container("beta"));
    assert_eq!(fs::read(w.p("dlc:beta")).unwrap(), container("beta"));
    assert_eq!(
        super::super::get_install(&w.e, &first.install_id)
            .unwrap()
            .status,
        "uninstalled"
    );
    w.e.rollback_transaction(&removed.id).unwrap();
    assert_eq!(fs::read(w.p("update")).unwrap(), container("alpha+beta"));
    w.delete(&first.install_id);
    w.delete(&second.install_id);
    let attempt =
        w.e.plan_file_ops(
            vec![FileOp::Copy {
                src: Path::new(&w.g.root_path).join("GTA5.exe"),
                dst: Path::new(&w.g.root_path).join("update/update.rpf"),
            }],
            "after final uninstall".into(),
        )
        .unwrap();
    w.e.approve_plan(&attempt.id).unwrap();
    assert!(w.e.apply_file_ops(&attempt.id).is_err());
    assert!(!w.p("update").exists());
    assert_eq!(
        fs::read(Path::new(&w.g.root_path).join("update/update.rpf")).unwrap(),
        container("vanilla")
    );
}
#[test]
fn gta_rpf_gameconfig_restores_exact_prior_copy_and_freezes_output() {
    let w = World::new();
    let dst = w.p("update");
    fs::create_dir_all(dst.parent().unwrap()).unwrap();
    let base = container("user edits");
    fs::write(&dst, &base).unwrap();
    let a = w.import("gameconfig.xml", b"<fwAllConfigs/>");
    let j = w.prep(&a, "gameconfig", json!({})).unwrap();
    w.publish(&j);
    let p = plan_install(&w.e, &w.g.id, &a, &j, None, &w.c).unwrap();
    let old_output = jobs(&w.e).unwrap().join(j).join("output");
    assert!(!old_output.parent().unwrap().exists());
    fs::create_dir_all(&old_output).unwrap();
    fs::write(old_output.join("update.rpf"), container("changed")).unwrap();
    w.commit(&p.plan);
    assert_eq!(fs::read(&dst).unwrap(), container("gameconfig"));
    w.delete(&p.install_id);
    assert_eq!(fs::read(dst).unwrap(), base);
}
#[test]
fn gta_rpf_rejects_unknown_result_fields_hardlinks_and_stale_plans() {
    let w = World::new();
    let a = w.import("dlc.rpf", &container("a"));
    let b = w.import("dlc.rpf", &container("b"));
    let j = w.prep(&a, "dlc", json!({"packName":"a"})).unwrap();
    w.publish(&j);
    let dir = jobs(&w.e).unwrap().join(&j);
    let result = dir.join("result.json");
    let valid = fs::read(&result).unwrap();
    let mut bad: Value = serde_json::from_slice(&valid).unwrap();
    bad["outputs"][0]["destination"] = json!("GTA5.exe");
    fs::write(&result, bad.to_string()).unwrap();
    assert!(plan_install(&w.e, &w.g.id, &a, &j, None, &w.c).is_err());
    fs::write(&result, &valid).unwrap();
    let temp = w._temp.path().join("hardlink");
    fs::hard_link(dir.join("output/update.rpf"), &temp).unwrap();
    assert!(plan_install(&w.e, &w.g.id, &a, &j, None, &w.c).is_err());
    fs::remove_file(temp).unwrap();
    let stale = plan_install(&w.e, &w.g.id, &a, &j, None, &w.c).unwrap();
    w.apply_new(&b, "dlc", json!({"packName":"b"}));
    w.e.approve_plan(&stale.plan.id).unwrap();
    assert!(w.e.apply_file_ops(&stale.plan.id).is_err());
    assert!(!w.p("dlc:a").exists());
}
#[test]
fn gta_rpf_probe_requires_same_selected_artifact_and_rejects_reserved_names() {
    let w = World::new();
    let a = w.import("dlc.rpf", &container("car"));
    let b = w.import("dlc.rpf", &container("different"));
    let id = w.prep(&a, "dlc", json!({})).unwrap();
    let job = frozen(&w.e, &id).unwrap();
    assert!(job.input.inspect_dlc.is_some() && job.input.targets.is_empty());
    let out = jobs(&w.e).unwrap().join(&id).join("result.json");
    for name in ["../outside", "CON", "gMm", "with/slash"] {
        fs::write(
            &out,
            json!({"schemaVersion":1,"jobId":id,"status":"inspected","packName":name}).to_string(),
        )
        .unwrap();
        assert!(w.prep(&a, "dlc", json!({"inspectJobId":id})).is_err());
    }
    fs::write(
        &out,
        json!({"schemaVersion":1,"jobId":id,"status":"inspected","packName":"car"}).to_string(),
    )
    .unwrap();
    assert!(w.prep(&b, "dlc", json!({"inspectJobId":id})).is_err());
    assert!(plan_install(&w.e, &w.g.id, &a, &id, None, &w.c).is_err());
    let actual = w.prep(&a, "dlc", json!({"inspectJobId":id})).unwrap();
    assert!(!jobs(&w.e).unwrap().join(&id).exists());
    assert!(frozen(&w.e, &actual)
        .unwrap()
        .item
        .keys()
        .contains(&"dlc:car".into()));
}
#[test]
fn gta_rpf_blocks_conflicts_drift_and_generic_parent_moves() {
    let w = World::new();
    let a = w.import("dlc.rpf", &container("a"));
    let path = w.p("dlc:foo");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, container("user-owned")).unwrap();
    assert!(w.prep(&a, "dlc", json!({"packName":"foo"})).is_err());
    let first = w.apply_new(&a, "dlc", json!({"packName":"alpha"}));
    let op = FileOp::Move {
        src: w.p("update").parent().unwrap().into(),
        dst: w.p("update").parent().unwrap().with_file_name("parked"),
    };
    let generic =
        w.e.plan_file_ops(vec![op], "directory move".into())
            .unwrap();
    assert!(guard_foreign_plan(&w.e, &generic).is_err());
    w.e.approve_plan(&generic.id).unwrap();
    assert!(w.e.apply_file_ops(&generic.id).is_err());
    fs::write(w.p("update"), container("external change")).unwrap();
    assert!(w.remove(&first.install_id).is_err());
    assert!(w.e.rollback_transaction(&first.plan.id).is_err());
}
#[test]
fn gta_rpf_failed_transaction_does_not_activate_or_keep_partial_dlc() {
    let w = World::new();
    let a = w.import("dlc.rpf", &container("a"));
    let j = w.prep(&a, "dlc", json!({"packName":"a"})).unwrap();
    w.publish(&j);
    let pre = plan_install(&w.e, &w.g.id, &a, &j, None, &w.c).unwrap();
    w.e.inject_failure_before(1);
    w.e.approve_plan(&pre.plan.id).unwrap();
    assert!(w.e.apply_file_ops(&pre.plan.id).is_err());
    assert!(head(&w.e, &w.g.id).unwrap().1.items.is_empty());
    assert!(!w.p("update").exists());
    assert!(!w.p("dlc:a").exists());
}

fn dummy_plan(op: FileOp) -> FilePlan {
    FilePlan {
        id: "synthetic-check".into(),
        summary: String::new(),
        total_bytes: 0,
        created_at: String::new(),
        requires_review: false,
        entries: vec![crate::transaction::PlannedEntry {
            operation: op,
            before: None,
            source: None,
            impact: String::new(),
        }],
    }
}
#[test]
fn gta_rpf_guards_original_and_existing_private_ancestors_without_blocking_ordinary_reads() {
    let w = World::new();
    let root = Path::new(&w.g.root_path);
    for slug in ["gta5-legacy", "gta5-enhanced"] {
        w.e.db
            .conn()
            .execute(
                "UPDATE games SET adapter_slug=?1 WHERE id=?2",
                params![slug, w.g.id],
            )
            .unwrap();
        let original = root.join("Update/UPDATE.rpf");
        assert!(guard_foreign_plan(
            &w.e,
            &dummy_plan(FileOp::WriteText {
                dst: original.clone(),
                content: "x".into()
            })
        )
        .is_err());
        assert!(guard_foreign_plan(
            &w.e,
            &dummy_plan(FileOp::Move {
                src: root.join("upDAte"),
                dst: root.join("renamed")
            })
        )
        .is_err());
        assert!(guard_foreign_plan(
            &w.e,
            &dummy_plan(FileOp::Copy {
                src: original,
                dst: root.join("copy.bak")
            })
        )
        .is_ok());
    }
    let private = store(&w.e);
    let ancestor = private.parent().unwrap().to_path_buf();
    let would_move = dummy_plan(FileOp::Move {
        src: ancestor,
        dst: w._temp.path().join("elsewhere"),
    });
    assert!(guard_foreign_plan(&w.e, &would_move).is_ok());
    fs::create_dir(private).unwrap();
    assert!(guard_foreign_plan(&w.e, &would_move).is_err());
    let tools = jobs(&w.e).unwrap().with_file_name("gta-rpf-tools");
    assert!(guard_foreign_plan(
        &w.e,
        &dummy_plan(FileOp::WriteText {
            dst: tools.join("rpf.dll"),
            content: String::new()
        })
    )
    .is_err());
    assert!(w.prep("missing", "dlc", json!({})).is_err());
}
#[test]
fn gta_rpf_parallel_reviews_same_artifact_can_only_commit_once() {
    let w = World::new();
    let a = w.import("dlc.rpf", &container("a"));
    let j1 = w.prep(&a, "dlc", json!({"packName":"a"})).unwrap();
    let j2 = w.prep(&a, "dlc", json!({"packName":"a"})).unwrap();
    w.publish(&j1);
    w.publish(&j2);
    let p1 = plan_install(&w.e, &w.g.id, &a, &j1, None, &w.c).unwrap();
    let p2 = plan_install(&w.e, &w.g.id, &a, &j2, None, &w.c).unwrap();
    w.commit(&p2.plan);
    w.e.approve_plan(&p1.plan.id).unwrap();
    assert!(w.e.apply_file_ops(&p1.plan.id).is_err());
    assert_eq!(head(&w.e, &w.g.id).unwrap().1.items.len(), 1);
}
#[test]
fn gta_rpf_vehicle_and_streamed_ped_rebuild_gmm_independently() {
    let w = World::new();
    let assets = crate::install::tests::zip(&w.e, &[("Car/a.yft", b"mesh"), ("Car/a.ytd", b"tex")]);
    let a = w.import("cars.zip", &fs::read(assets).unwrap());
    let assets = crate::install::tests::zip(
        &w.e,
        &[
            ("Peds/ada.ymt", b"meta"),
            ("Peds/ada/shirt_000.ydd", b"component"),
        ],
    );
    let b = w.import("peds.zip", &fs::read(assets).unwrap());
    let car = w.apply_new(&a, "vehicle", json!({"sourceDir":"Car"}));
    let ped = w.apply_new(
        &b,
        "ped",
        json!({"modelName":"Ada","streamed":true,"sex":"Female"}),
    );
    assert_eq!(fs::read(w.p("gmm")).unwrap(), container("vehicle+ped"));
    w.delete(&car.install_id);
    assert_eq!(fs::read(w.p("gmm")).unwrap(), container("ped"));
    w.delete(&ped.install_id);
    assert!(!w.p("gmm").exists());
}

#[test]
fn gta_rpf_cleanup_never_deletes_replaced_workspace() {
    let w = World::new();
    let a = w.import("dlc.rpf", &container("a"));
    let j = w.prep(&a, "dlc", json!({"packName":"a"})).unwrap();
    let f = frozen(&w.e, &j).unwrap();
    let path = jobs(&w.e).unwrap().join(&j);
    let moved = path.with_file_name("original");
    fs::rename(&path, &moved).unwrap();
    fs::create_dir(&path).unwrap();
    fs::write(path.join("unrelated"), b"must remain").unwrap();
    clean_consumed(&w.e, &f);
    assert!(moved.join("input.json").exists());
    assert_eq!(fs::read(path.join("unrelated")).unwrap(), b"must remain");
}

#[test]
fn gta_rpf_library_and_specialized_pending_reviews_exclude_each_other_at_apply() {
    use super::super::library::{plan_library_install, LibraryInstallRequest};
    for ordinary_wins in [true, false] {
        let w = World::new();
        let assets = crate::install::tests::zip(
            &w.e,
            &[("dlc.rpf", &container("addon")), ("notes.txt", b"manual")],
        );
        let a = w.import("car.zip", &fs::read(assets).unwrap());
        let request: LibraryInstallRequest = serde_json::from_value(json!({"gameId": w.g.id, "artifactId": a, "mode":"approvedMappings", "sourceKind":"archive", "mappings":[{"source":"notes.txt","destination":"test-notes.txt"}]})).unwrap();
        let j = w.prep(&a, "dlc", json!({"packName":"addon"})).unwrap();
        w.publish(&j);
        let rpf = plan_install(&w.e, &w.g.id, &a, &j, None, &w.c).unwrap();
        let ordinary = plan_library_install(&w.e, &request).unwrap();
        let (first, second) = if ordinary_wins {
            (&ordinary, &rpf)
        } else {
            (&rpf, &ordinary)
        };
        w.commit(&first.plan);
        w.e.approve_plan(&second.plan.id).unwrap();
        assert!(w.e.apply_file_ops(&second.plan.id).is_err());
        assert_eq!(w.p("dlc:addon").exists(), !ordinary_wins);
        assert_eq!(
            Path::new(&w.g.root_path).join("test-notes.txt").exists(),
            ordinary_wins
        );
        if ordinary_wins {
            let uninstalled = super::super::plan_uninstall(&w.e, &ordinary.install_id).unwrap();
            w.commit(&uninstalled);
            w.apply_new(&a, "dlc", json!({"packName":"addon"}));
            assert!(w.e.rollback_transaction(&uninstalled.id).is_err());
            let item = crate::trash::list_trash(&w.e.db, false)
                .unwrap()
                .into_iter()
                .find(|t| t.transaction_id == uninstalled.id)
                .unwrap();
            let restore = crate::trash::plan_restore(&w.e, &[item.id], false).unwrap();
            w.e.approve_plan(&restore.id).unwrap();
            assert!(w.e.apply_file_ops(&restore.id).is_err());
            assert!(!Path::new(&w.g.root_path).join("test-notes.txt").exists());
        } else {
            let uninstalled = w.delete(&rpf.install_id);
            let replacement = plan_library_install(&w.e, &request).unwrap();
            w.commit(&replacement.plan);
            assert!(w.e.rollback_transaction(&uninstalled.id).is_err());
        }
    }
}

#[test]
fn gta_rpf_menyoo_restores_bytes_and_vendor_update_invalidates_job() {
    let w = World::new();
    let a = w.import("dlc.rpf", &container("pack"));
    assert!(w
        .prep(&a, "dlc", json!({"packName":"pack", "menyoo":true}))
        .is_err());
    let p = w.p("menyoo");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    let baseline = b"<AddedVehicleModels><!-- keep original formatting --></AddedVehicleModels>";
    fs::write(&p, baseline).unwrap();
    let installed = w.apply_new(&a, "dlc", json!({"packName":"pack", "menyoo":true}));
    assert_ne!(fs::read(&p).unwrap(), baseline);
    let j = w.remove(&installed.install_id).unwrap();
    w.publish(&j);
    let uninstall = plan_uninstall(&w.e, &w.g.id, &installed.install_id, &j, &w.c).unwrap();
    let original = Path::new(&w.g.root_path).join("update/update.rpf");
    fs::write(&original, container("game update")).unwrap();
    w.e.approve_plan(&uninstall.id).unwrap();
    assert!(w.e.apply_file_ops(&uninstall.id).is_err());
    assert!(
        !super::super::inspect_install(&w.e, &installed.install_id)
            .unwrap()
            .can_uninstall
    );
    fs::write(original, container("vanilla")).unwrap();
    w.delete(&installed.install_id);
    assert_eq!(fs::read(p).unwrap(), baseline);
}

#[test]
fn gta_rpf_enhanced_installs_and_uninstalls_gameconfig_and_inspected_dlc() {
    let w = World::for_edition(Edition::Enhanced);
    let root = Path::new(&w.g.root_path);
    let baseline = root.join("update/update.rpf");
    let marker = root.join("GTA5_Enhanced.exe");
    let config = w.import("gameconfig.xml", b"<fwAllConfigs/>");
    let dlc = w.import("dlc.rpf", &container("enhanced dlc source"));

    for (artifact, mod_type) in [(&config, "rpf-gameconfig"), (&dlc, "rpf-dlc")] {
        let matched = super::super::match_library_install(
            &w.e,
            &super::super::LibraryMatchRequest {
                game_id: w.g.id.clone(),
                library_entry_id: artifact.clone(),
            },
        )
        .unwrap();
        assert!(matched.candidates.iter().any(|candidate| {
            candidate.mod_type == mod_type && candidate.status == "requiresGtaRpf"
        }));
    }

    let config_job = w.prep(&config, "gameconfig", json!({})).unwrap();
    let job_root = jobs(&w.e).unwrap();
    let input = |id: &str| -> Value {
        serde_json::from_slice(&fs::read(job_root.join(id).join("input.json")).unwrap()).unwrap()
    };
    assert_eq!(input(&config_job)["kind"], "enhanced");
    assert_eq!(input(&config_job)["targets"][0]["id"], "update");
    assert!(!w.p("update").exists());
    w.publish(&config_job);
    let preview = super::super::plan_library_install(
        &w.e,
        &serde_json::from_value(json!({
            "gameId":w.g.id, "artifactId":config, "mode":"gtaRpf", "jobId":config_job
        }))
        .unwrap(),
    )
    .unwrap();
    w.commit(&preview.plan);
    assert_eq!(fs::read(w.p("update")).unwrap(), container("gameconfig"));
    assert!(guard_foreign_plan(
        &w.e,
        &dummy_plan(FileOp::WriteText {
            dst: marker.clone(),
            content: "foreign write".into(),
        })
    )
    .is_err());

    let probe = w.prep(&dlc, "dlc", json!({})).unwrap();
    assert_eq!(input(&probe)["kind"], "enhanced");
    assert_eq!(input(&probe)["targets"], json!([]));
    assert!(input(&probe)["inspectDlc"].is_object());
    fs::write(
        job_root.join(&probe).join("result.json"),
        json!({"schemaVersion":1,"jobId":probe,"status":"inspected","packName":"enh_pack"})
            .to_string(),
    )
    .unwrap();
    let job = w
        .prep(&dlc, "dlc", json!({"inspectJobId":probe, "menyoo":false}))
        .unwrap();
    assert_eq!(input(&job)["kind"], "enhanced");
    assert!(input(&job).get("inspectDlc").is_none());
    assert_eq!(input(&job)["targets"].as_array().unwrap().len(), 1);
    assert_eq!(
        input(&job)["targets"][0]["contributions"][1]["packName"],
        "enh_pack"
    );
    w.publish(&job);
    let dlc_preview = plan_install(&w.e, &w.g.id, &dlc, &job, None, &w.c).unwrap();
    w.commit(&dlc_preview.plan);
    assert_eq!(
        fs::read(w.p("update")).unwrap(),
        container("gameconfig+enh_pack")
    );
    assert_eq!(
        fs::read(w.p("dlc:enh_pack")).unwrap(),
        container("enhanced dlc source")
    );

    let remove = w.remove(&preview.install_id).unwrap();
    assert_eq!(input(&remove)["kind"], "enhanced");
    w.publish(&remove);
    let removal = plan_uninstall(&w.e, &w.g.id, &preview.install_id, &remove, &w.c).unwrap();
    w.commit(&removal);
    assert_eq!(fs::read(w.p("update")).unwrap(), container("enh_pack"));
    w.delete(&dlc_preview.install_id);
    assert!(!w.p("update").exists());
    assert!(!w.p("dlc:enh_pack").exists());
    assert!(!w.p("gmm").exists());
    assert!(!w.p("menyoo").exists());
    assert_eq!(fs::read(&baseline).unwrap(), container("vanilla"));
    assert_eq!(fs::read(&marker).unwrap(), b"GTA5_Enhanced.exe");
}

#[test]
fn gta_rpf_enhanced_rejects_unverified_jobs_and_ambiguous_markers() {
    let w = World::for_edition(Edition::Enhanced);
    let vehicle = w.import("car.yft", b"vehicle");
    let matched = super::super::match_library_install(
        &w.e,
        &super::super::LibraryMatchRequest {
            game_id: w.g.id.clone(),
            library_entry_id: vehicle.clone(),
        },
    )
    .unwrap();
    assert!(matched.candidates.iter().any(|candidate| {
        candidate.mod_type == "rpf-vehicle" && candidate.status == "unsupportedEdition"
    }));
    let ped = w.import("ped.ydd", b"ped");
    let dlc = w.import("dlc.rpf", &container("unverified Menyoo contribution"));
    for (artifact, kind, options) in [
        (&vehicle, "vehicle", json!({})),
        (&ped, "ped", json!({"modelName":"ped","sex":"Male"})),
        (&dlc, "dlc", json!({"menyoo":true})),
    ] {
        assert_eq!(
            w.prep(artifact, kind, options)
                .unwrap_err()
                .presentation()
                .key,
            "gta.rpf.enhanced_unsupported"
        );
    }
    assert!(!jobs(&w.e).unwrap().exists());

    let legacy_marker = Path::new(&w.g.root_path).join("GTA5.exe");
    fs::write(&legacy_marker, b"another edition").unwrap();
    assert!(matches!(
        w.prep(&dlc, "dlc", json!({})),
        Err(AppError::Conflict(_))
    ));
    fs::remove_file(legacy_marker).unwrap();
    w.e.db
        .conn()
        .execute(
            "UPDATE games SET steam_app_id='271590' WHERE id=?1",
            [&w.g.id],
        )
        .unwrap();
    assert!(matches!(
        w.prep(&dlc, "dlc", json!({})),
        Err(AppError::Conflict(_))
    ));
    assert!(!jobs(&w.e).unwrap().exists());
}

#[test]
fn gta_rpf_enhanced_rejects_oversize_receipts_and_marker_drift_after_plan() {
    let w = World::for_edition(Edition::Enhanced);
    let config = w.import("gameconfig.xml", b"<fwAllConfigs/>");
    let id = w.prep(&config, "gameconfig", json!({})).unwrap();
    w.publish(&id);
    let result_path = jobs(&w.e).unwrap().join(&id).join("result.json");
    let valid_result = fs::read(&result_path).unwrap();
    let mut result: Value = serde_json::from_slice(&valid_result).unwrap();
    result["outputs"][0]["size"] = json!(4_u64 * 1024_u64.pow(3) + 1);
    fs::write(&result_path, result.to_string()).unwrap();
    assert!(matches!(
        plan_install(&w.e, &w.g.id, &config, &id, None, &w.c),
        Err(AppError::Invalid(reason)) if reason.contains("尺寸")
    ));
    assert!(!w.p("update").exists());

    fs::write(&result_path, valid_result).unwrap();
    let preview = plan_install(&w.e, &w.g.id, &config, &id, None, &w.c).unwrap();
    w.e.approve_plan(&preview.plan.id).unwrap();
    fs::write(
        Path::new(&w.g.root_path).join("GTA5_Enhanced.exe"),
        b"changed marker",
    )
    .unwrap();
    assert!(matches!(
        w.e.apply_file_ops(&preview.plan.id),
        Err(AppError::Conflict(_))
    ));
    assert!(!w.p("update").exists());
}

#[test]
fn gta_rpf_archive_job_snapshots_the_requested_path_for_both_editions() {
    for (edition, wire_edition) in [(Edition::Legacy, "legacy"), (Edition::Enhanced, "enhanced")] {
        let w = World::for_edition(edition);
        let root = Path::new(&w.g.root_path);
        let path = "mods/update/update.rpf";
        let Err(AppError::NotFound(reason)) = prepare_archive_job(&w.e, &w.g.id, path, &[], &w.c)
        else {
            panic!("missing mods copy must be reported even when the original exists");
        };
        assert!(reason.contains(path));
        let modified = container("existing mods copy");
        fs::create_dir_all(root.join("mods/update")).unwrap();
        fs::write(root.join(path), &modified).unwrap();
        fs::write(root.join("mods/replacement.xml"), b"replacement").unwrap();
        fs::write(root.join("mods/empty.txt"), b"").unwrap();
        let result = serde_json::to_value(
            prepare_archive_job(
                &w.e,
                &w.g.id,
                r"mods\update\update.rpf",
                &[r"mods\replacement.xml".into(), "mods/empty.txt".into()],
                &w.c,
            )
            .unwrap(),
        )
        .unwrap();
        let job_id = result["jobId"].as_str().unwrap();
        assert!(digest_ok(job_id, 32));
        assert_eq!(result["gameId"], w.g.id);
        assert_eq!(result["edition"], wire_edition);
        assert_eq!(result["archivePath"], path);
        assert_eq!(
            result["archive"],
            json!({"path":"inputs/archive", "name":"update.rpf"})
        );
        assert_eq!(
            result["payloads"],
            json!([
                {"gamePath":"mods/replacement.xml", "sourcePath":"inputs/payload-0"},
                {"gamePath":"mods/empty.txt", "sourcePath":"inputs/payload-1"}
            ])
        );
        let job_dir = jobs(&w.e).unwrap().join(job_id);
        let wire: Value =
            serde_json::from_slice(&fs::read(job_dir.join("input.json")).unwrap()).unwrap();
        let file_ref = |path: &str, data: &[u8]| {
            assert_eq!(fs::read(job_dir.join(path)).unwrap(), data);
            json!({"path":path,"sha256":hex::encode(Sha256::digest(data)),"size":data.len()})
        };
        assert_eq!(
            wire,
            json!({
                "schemaVersion":1,"jobId":job_id,"kind":wire_edition,
                "archiveInputs":[{"name":"update.rpf","source":file_ref("inputs/archive", &modified)}],
                "payloadInputs":[file_ref("inputs/payload-0", b"replacement"),file_ref("inputs/payload-1", b"")]
            })
        );
        assert_eq!(fs::read(root.join(path)).unwrap(), modified);
        assert_eq!(
            fs::read(root.join("update/update.rpf")).unwrap(),
            container("vanilla")
        );
        assert!(
            matches!(frozen(&w.e, job_id), Err(AppError::Invalid(reason)) if reason == "未知本机 RPF 任务")
        );
        assert!(matches!(
            plan_install(&w.e, &w.g.id, "artifact", job_id, None, &w.c),
            Err(AppError::Invalid(reason)) if reason == "未知本机 RPF 任务"
        ));
        assert!(matches!(
            plan_uninstall(&w.e, &w.g.id, "install", job_id, &w.c),
            Err(AppError::Invalid(reason)) if reason == "未知本机 RPF 任务"
        ));
    }
}

#[test]
fn gta_rpf_archive_job_rejects_unsafe_paths_and_payload_sources() {
    let w = World::new();
    let root = Path::new(&w.g.root_path);
    let path = "update/update.rpf";
    fs::write(root.join(".env"), b"private").unwrap();
    fs::hard_link(root.join(path), root.join("alias.rpf")).unwrap();
    assert!(prepare_archive_job(&w.e, &w.g.id, path, &[], &w.c).is_err());
    fs::remove_file(root.join("alias.rpf")).unwrap();
    for candidate in [
        "../update/update.rpf",
        r"C:\game\update.rpf",
        "update/update.rpf/common/data/dlclist.xml",
        "update.rpf:stream",
    ] {
        assert!(
            prepare_archive_job(&w.e, &w.g.id, candidate, &[], &w.c).is_err(),
            "{candidate}"
        );
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("update"), root.join("linked")).unwrap();
    #[cfg(unix)]
    assert!(prepare_archive_job(&w.e, &w.g.id, "linked/update.rpf", &[], &w.c).is_err());
    fs::write(root.join("bad.rpf"), [0; 64]).unwrap();
    assert!(prepare_archive_job(&w.e, &w.g.id, "bad.rpf", &[], &w.c).is_err());
    for payload in [
        ".env",
        "../outside.xml",
        r"C:\game\payload.xml",
        "update",
        "missing.xml",
    ] {
        assert!(
            prepare_archive_job(&w.e, &w.g.id, path, &[payload.into()], &w.c).is_err(),
            "{payload}"
        );
    }
    w.c.cancel();
    assert!(matches!(
        prepare_archive_job(&w.e, &w.g.id, path, &[], &w.c),
        Err(AppError::Cancelled)
    ));
    assert!(!jobs(&w.e).unwrap().exists());
}
