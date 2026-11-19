use super::*;

const ROOT: &str = "/v2/version/Root0001";
const GAME_VERSION: &str = "1.21.1";
const LOADER: &str = "fabric";

fn request() -> DependencyGraphRequest {
    DependencyGraphRequest {
        project_id: None,
        version_id: "Root0001".into(),
        game_version: GAME_VERSION.into(),
        loader: LOADER.into(),
        project_pins: Default::default(),
    }
}

fn source(steps: Vec<Step>) -> (Sources, MockApi) {
    let (provider, mock) = provider("modrinth", steps);
    (
        Sources {
            providers: vec![Arc::new(provider)],
        },
        mock,
    )
}

fn version(id: &str, project: &str, declarations: Vec<Value>) -> Value {
    // Graph metadata does not require any file or download fields.
    json!({
        "id": id,
        "project_id": project,
        "game_versions": [GAME_VERSION],
        "loaders": [LOADER],
        "version_type": "release",
        "dependencies": declarations,
    })
}

fn dep(kind: &str, project: Option<&str>, version: Option<&str>, file: Option<&str>) -> Value {
    json!({
        "dependency_type": kind,
        "project_id": project,
        "version_id": version,
        "file_name": file,
    })
}

fn required(project: &str, version: Option<&str>) -> Value {
    dep("required", Some(project), version, None)
}

fn exact(path: &'static str, project: &str, declarations: Vec<Value>) -> Step {
    let id = path.strip_prefix("/v2/version/").unwrap();
    Step::ok(path, version(id, project, declarations).to_string())
}

async fn read_graph(req: DependencyGraphRequest, steps: Vec<Step>) -> (Value, MockApi) {
    let count = steps.len();
    let (sources, mock) = source(steps);
    let graph = sources.modrinth_dependency_graph(&req).await.unwrap();
    mock.assert_finished(count);
    let graph = serde_json::to_value(graph).unwrap();
    assert_eq!(graph["root_version_id"], req.version_id);
    (graph, mock)
}

fn rows<'a>(graph: &'a Value, key: &str) -> &'a [Value] {
    graph[key].as_array().unwrap()
}

fn node<'a>(graph: &'a Value, id: &str) -> &'a Value {
    rows(graph, "nodes")
        .iter()
        .find(|node| node["version_id"] == id)
        .unwrap_or_else(|| panic!("missing node {id}"))
}

fn edge<'a>(graph: &'a Value, from: &str, declaration: &Value) -> &'a Value {
    rows(graph, "edges")
        .iter()
        .find(|edge| edge["from_version_id"] == from && edge["declaration"] == *declaration)
        .unwrap_or_else(|| panic!("missing edge from {from}: {declaration}"))
}

#[tokio::test]
async fn modrinth_dependency_graph_expands_pinned_diamond_breadth_first_once() {
    let left = required("LeftProj", Some("Left0001"));
    let right = required("RightPrj", Some("Right001"));
    let shared = required("SharedPr", Some("Shared01"));
    let shared_by_version = dep("required", None, Some("Shared01"), None);
    let leaf = required("LeafProj", Some("Leaf0001"));
    let (graph, _) = read_graph(
        request(),
        vec![
            exact(ROOT, "RootProj", vec![left.clone(), right.clone()]),
            exact("/v2/version/Left0001", "LeftProj", vec![shared.clone()]),
            exact(
                "/v2/version/Right001",
                "RightPrj",
                vec![shared_by_version.clone()],
            ),
            exact("/v2/version/Shared01", "SharedPr", vec![leaf.clone()]),
            exact("/v2/version/Leaf0001", "LeafProj", vec![]),
        ],
    )
    .await;
    assert_eq!(graph["complete"], true);
    assert_eq!(graph["truncated"], false);
    assert!(rows(&graph, "issues").is_empty());
    assert_eq!(rows(&graph, "nodes").len(), 5);
    assert_eq!(rows(&graph, "edges").len(), 5);
    for (id, project) in [
        ("Root0001", "RootProj"),
        ("Left0001", "LeftProj"),
        ("Right001", "RightPrj"),
        ("Shared01", "SharedPr"),
        ("Leaf0001", "LeafProj"),
    ] {
        let node = node(&graph, id);
        assert_eq!(node["project_id"], project);
        assert_eq!(node["declarations_known"], true);
    }
    for (from, declaration, selected) in [
        ("Root0001", left, "Left0001"),
        ("Root0001", right, "Right001"),
        ("Left0001", shared, "Shared01"),
        ("Right001", shared_by_version, "Shared01"),
        ("Shared01", leaf, "Leaf0001"),
    ] {
        let edge = edge(&graph, from, &declaration);
        assert_eq!(edge["state"], "selected");
        assert_eq!(edge["selected_version_id"], selected);
    }
}

#[tokio::test]
async fn modrinth_dependency_graph_never_fetches_non_required_or_filename_references() {
    let declarations = [
        (
            dep("optional", Some("Optional"), None, None),
            "not_followed",
        ),
        (
            dep("embedded", None, Some("Embedded"), Some("inside.jar")),
            "not_followed",
        ),
        (
            dep("incompatible", None, Some("Conflict"), None),
            "not_followed",
        ),
        (
            dep("required", None, None, Some("manual.jar")),
            "reference_only",
        ),
    ];
    let (graph, _) = read_graph(
        request(),
        vec![exact(
            ROOT,
            "RootProj",
            declarations.iter().map(|(dep, _)| dep.clone()).collect(),
        )],
    )
    .await;
    assert_eq!(rows(&graph, "nodes").len(), 1);
    assert_eq!(rows(&graph, "edges").len(), declarations.len());
    assert!(rows(&graph, "issues").is_empty());
    for (declaration, state) in declarations {
        let edge = edge(&graph, "Root0001", &declaration);
        assert_eq!(edge["state"], state);
        assert!(edge["selected_version_id"].is_null());
        assert!(rows(edge, "candidates").is_empty());
    }
}

#[tokio::test]
async fn modrinth_dependency_graph_project_only_uses_one_compatible_release_and_rechecks_exact() {
    let declaration = required("ChildPrj", None);
    let release = version("Release1", "ChildPrj", vec![]);
    let mut wrong_game = version("WrongGam", "ChildPrj", vec![]);
    wrong_game["game_versions"] = json!(["1.20.1"]);
    let mut wrong_loader = version("WrongLdr", "ChildPrj", vec![]);
    wrong_loader["loaders"] = json!(["forge"]);
    let (graph, mock) = read_graph(
        request(),
        vec![
            exact(ROOT, "RootProj", vec![declaration.clone()]),
            Step::ok(
                "/v2/project/ChildPrj/version",
                json!([wrong_game, release, wrong_loader]).to_string(),
            ),
            exact(
                "/v2/version/Release1",
                "ChildPrj",
                vec![required("LeafProj", Some("Leaf0001"))],
            ),
            exact("/v2/version/Leaf0001", "LeafProj", vec![]),
        ],
    )
    .await;
    let params = mock.params(1);
    assert_eq!(value(&params["game_versions"]), json!([GAME_VERSION]));
    assert_eq!(value(&params["loaders"]), json!([LOADER]));
    let chosen = edge(&graph, "Root0001", &declaration);
    assert_eq!(chosen["state"], "selected");
    assert_eq!(chosen["selected_version_id"], "Release1");
    assert_eq!(node(&graph, "Leaf0001")["project_id"], "LeafProj");
    assert_eq!(graph["complete"], true);
}

#[tokio::test]
async fn modrinth_dependency_graph_reports_project_choices_without_fetching_candidates() {
    let first = version("Release1", "ChildPrj", vec![]);
    let second = version("Release2", "ChildPrj", vec![]);
    let mut beta = version("Beta0001", "ChildPrj", vec![]);
    beta["version_type"] = json!("beta");
    let declaration = required("ChildPrj", None);
    for (listing, state, expected) in [
        (
            json!([second, &first]),
            "choice_required",
            vec!["Release1", "Release2"],
        ),
        (
            json!([&beta, first]),
            "choice_required",
            vec!["Beta0001", "Release1"],
        ),
        (json!([beta]), "choice_required", vec!["Beta0001"]),
        (json!([]), "no_compatible_version", vec![]),
    ] {
        let (graph, _) = read_graph(
            request(),
            vec![
                exact(ROOT, "RootProj", vec![declaration.clone()]),
                Step::ok("/v2/project/ChildPrj/version", listing.to_string()),
            ],
        )
        .await;
        assert_eq!(rows(&graph, "nodes").len(), 1);
        assert_eq!(graph["complete"], false);
        assert_eq!(graph["truncated"], false);
        let edge = edge(&graph, "Root0001", &declaration);
        assert_eq!(edge["state"], state);
        assert!(edge["selected_version_id"].is_null());
        let mut candidates: Vec<_> = rows(edge, "candidates")
            .iter()
            .map(|candidate| candidate.as_str().unwrap())
            .collect();
        candidates.sort_unstable();
        assert_eq!(candidates, expected);
    }
}

#[tokio::test]
async fn modrinth_dependency_graph_project_pins_skip_list_but_not_exact_validation() {
    let pinned = required("PinnedPr", None);
    let fixed = required("FixedPrj", Some("FixedVer"));
    let wrong_project = required("Expected", None);
    let incompatible = required("OldProj1", None);
    let wrong_id = required("WrongPrj", Some("Want0001"));
    let optional = dep("optional", Some("OptProj1"), None, None);
    let mut old = version("Old00001", "OldProj1", vec![]);
    old["loaders"] = json!(["forge"]);
    let (graph, _) = read_graph(
        DependencyGraphRequest {
            project_id: Some("RootProj".into()),
            project_pins: BTreeMap::from([
                ("PinnedPr".into(), "Pick0001".into()),
                ("FixedPrj".into(), "Ignored1".into()),
                ("Expected".into(), "WrongPrj".into()),
                ("OldProj1".into(), "Old00001".into()),
                ("OptProj1".into(), "Ignored2".into()),
            ]),
            ..request()
        },
        vec![
            exact(
                ROOT,
                "RootProj",
                vec![
                    pinned.clone(),
                    fixed.clone(),
                    wrong_project.clone(),
                    incompatible.clone(),
                    wrong_id.clone(),
                    optional.clone(),
                ],
            ),
            exact("/v2/version/Pick0001", "PinnedPr", vec![]),
            exact("/v2/version/FixedVer", "FixedPrj", vec![]),
            exact("/v2/version/WrongPrj", "OtherPrj", vec![]),
            Step::ok("/v2/version/Old00001", old.to_string()),
            Step::ok(
                "/v2/version/Want0001",
                version("OtherVer", "WrongPrj", vec![]).to_string(),
            ),
        ],
    )
    .await;
    assert_eq!(graph["complete"], false);
    assert_eq!(rows(&graph, "nodes").len(), 3);
    for (declaration, selected) in [(pinned, "Pick0001"), (fixed, "FixedVer")] {
        let edge = edge(&graph, "Root0001", &declaration);
        assert_eq!(edge["state"], "selected");
        assert_eq!(edge["selected_version_id"], selected);
    }
    for (declaration, state) in [
        (wrong_project, "project_mismatch"),
        (incompatible, "incompatible_version"),
        (wrong_id, "metadata_unavailable"),
        (optional, "not_followed"),
    ] {
        let edge = edge(&graph, "Root0001", &declaration);
        assert_eq!(edge["state"], state);
        assert!(edge["selected_version_id"].is_null());
    }
}

#[tokio::test]
async fn modrinth_dependency_graph_distinguishes_unknown_declarations_from_empty() {
    let unknown = required("UnkProj1", Some("Unknown1"));
    let valid = required("KnownPrj", Some("Known001"));
    for unknown_declarations in [
        None,
        Some(Value::Null),
        Some(json!({})),
        Some(json!([{"dependency_type": "required", "project_id": 17}])),
    ] {
        let mut unknown_version = version("Unknown1", "UnkProj1", vec![]);
        if let Some(declarations) = unknown_declarations {
            unknown_version["dependencies"] = declarations;
        } else {
            unknown_version
                .as_object_mut()
                .unwrap()
                .remove("dependencies");
        }
        let (graph, _) = read_graph(
            request(),
            vec![
                exact(ROOT, "RootProj", vec![unknown.clone(), valid.clone()]),
                Step::ok("/v2/version/Unknown1", unknown_version.to_string()),
                exact("/v2/version/Known001", "KnownPrj", vec![]),
            ],
        )
        .await;
        assert_eq!(graph["complete"], false);
        assert_eq!(graph["truncated"], false);
        assert_eq!(rows(&graph, "nodes").len(), 3);
        assert_eq!(rows(&graph, "edges").len(), 2);
        assert_eq!(node(&graph, "Unknown1")["declarations_known"], false);
        assert_eq!(node(&graph, "Known001")["declarations_known"], true);
        assert_eq!(edge(&graph, "Root0001", &unknown)["state"], "selected");
    }
}

#[tokio::test]
async fn modrinth_dependency_graph_rejects_invalid_root_id_and_mismatched_exact_metadata() {
    let (sources, mock) = source(vec![]);
    for req in [
        DependencyGraphRequest {
            version_id: "invalid/id".into(),
            ..request()
        },
        DependencyGraphRequest {
            project_id: Some("invalid/project".into()),
            ..request()
        },
        DependencyGraphRequest {
            game_version: String::new(),
            ..request()
        },
        DependencyGraphRequest {
            loader: String::new(),
            ..request()
        },
    ] {
        assert!(sources.modrinth_dependency_graph(&req).await.is_err());
    }
    mock.assert_finished(0);

    let mut wrong_game = version("Root0001", "RootProj", vec![]);
    wrong_game["game_versions"] = json!(["1.20.1"]);
    let mut wrong_loader = version("Root0001", "RootProj", vec![]);
    wrong_loader["loaders"] = json!(["forge"]);
    let req = DependencyGraphRequest {
        project_id: Some("RootProj".into()),
        ..request()
    };
    for (problem, metadata) in [
        ("response ID", version("OtherVer", "RootProj", vec![])),
        ("response project", version("Root0001", "OtherPrj", vec![])),
        ("game version", wrong_game),
        ("loader", wrong_loader),
    ] {
        let (sources, mock) = source(vec![Step::ok(ROOT, metadata.to_string())]);
        assert!(
            sources.modrinth_dependency_graph(&req).await.is_err(),
            "accepted mismatched {problem}"
        );
        mock.assert_finished(1);
    }
}

#[tokio::test]
async fn modrinth_dependency_graph_reports_each_issue_as_incomplete() {
    let first = required("ChildPrj", Some("Child001"));
    for (kind, root_declarations, child_declarations, second) in [
        (
            "cycle",
            vec![first.clone()],
            vec![required("RootProj", Some("Root0001"))],
            None,
        ),
        (
            "conflicting_versions",
            vec![first.clone(), required("ChildPrj", Some("Child002"))],
            vec![],
            Some(exact("/v2/version/Child002", "ChildPrj", vec![])),
        ),
        (
            "declared_incompatible",
            vec![first.clone()],
            vec![dep("incompatible", Some("RootProj"), None, None)],
            None,
        ),
    ] {
        let mut steps = vec![
            exact(ROOT, "RootProj", root_declarations),
            exact("/v2/version/Child001", "ChildPrj", child_declarations),
        ];
        steps.extend(second);
        let (graph, _) = read_graph(request(), steps).await;
        assert!(
            rows(&graph, "issues")
                .iter()
                .any(|issue| issue["kind"] == kind),
            "missing {kind} issue: {}",
            graph["issues"]
        );
        assert_eq!(graph["complete"], false, "{kind}");
        assert_eq!(graph["truncated"], false, "{kind}");
    }
}

// The mock path has a static lifetime, so build both endpoint forms at compile time.
macro_rules! branch_paths {
    ($($id:literal),+ $(,)?) => {
        [$(($id, concat!("/v2/version/", $id), concat!("/v2/project/", $id, "/version"))),+]
    };
}

const BRANCHES: [(&str, &str, &str); 48] = branch_paths![
    "Branch01", "Branch02", "Branch03", "Branch04", "Branch05", "Branch06", "Branch07", "Branch08",
    "Branch09", "Branch10", "Branch11", "Branch12", "Branch13", "Branch14", "Branch15", "Branch16",
    "Branch17", "Branch18", "Branch19", "Branch20", "Branch21", "Branch22", "Branch23", "Branch24",
    "Branch25", "Branch26", "Branch27", "Branch28", "Branch29", "Branch30", "Branch31", "Branch32",
    "Branch33", "Branch34", "Branch35", "Branch36", "Branch37", "Branch38", "Branch39", "Branch40",
    "Branch41", "Branch42", "Branch43", "Branch44", "Branch45", "Branch46", "Branch47", "Branch48",
];

#[tokio::test]
async fn modrinth_dependency_graph_stops_at_depth_eight() {
    let mut steps = vec![exact(
        ROOT,
        "RootProj",
        vec![required(BRANCHES[0].0, Some(BRANCHES[0].0))],
    )];
    for (&(project, path, _), &(next, _, _)) in BRANCHES[..8].iter().zip(&BRANCHES[1..9]) {
        steps.push(exact(path, project, vec![required(next, Some(next))]));
    }
    let (graph, _) = read_graph(request(), steps).await;
    assert_eq!(rows(&graph, "nodes").len(), 9);
    assert_eq!(graph["complete"], false);
    assert_eq!(graph["truncated"], true);
    assert_eq!(
        edge(&graph, "Branch08", &required("Branch09", Some("Branch09")))["state"],
        "limit_reached"
    );
}

#[tokio::test]
async fn modrinth_dependency_graph_stops_at_thirty_two_nodes() {
    let mut steps = vec![exact(
        ROOT,
        "RootProj",
        BRANCHES[..32]
            .iter()
            .map(|&(id, _, _)| required(id, Some(id)))
            .collect(),
    )];
    steps.extend(
        BRANCHES[..31]
            .iter()
            .map(|&(id, path, _)| exact(path, id, vec![])),
    );
    let (graph, _) = read_graph(request(), steps).await;
    assert_eq!(rows(&graph, "nodes").len(), 32);
    assert_eq!(graph["complete"], false);
    assert_eq!(graph["truncated"], true);
    assert_eq!(
        edge(&graph, "Root0001", &required("Branch32", Some("Branch32")))["state"],
        "limit_reached"
    );
}

#[tokio::test]
async fn modrinth_dependency_graph_stops_at_two_hundred_fifty_six_edges() {
    let optional: Vec<_> = (0..128)
        .map(|index| dep("optional", Some(&format!("Option{index:03}")), None, None))
        .collect();
    let (graph, _) = read_graph(
        request(),
        vec![
            exact(
                ROOT,
                "RootProj",
                vec![
                    required("ChildPrA", Some("Child001")),
                    required("ChildPrB", Some("Child002")),
                ],
            ),
            exact("/v2/version/Child001", "ChildPrA", optional.clone()),
            exact("/v2/version/Child002", "ChildPrB", optional),
        ],
    )
    .await;
    assert_eq!(rows(&graph, "nodes").len(), 3);
    assert_eq!(rows(&graph, "edges").len(), 256);
    assert_eq!(graph["complete"], false);
    assert_eq!(graph["truncated"], true);
}

#[tokio::test]
async fn modrinth_dependency_graph_stops_at_forty_eight_requests() {
    let mut steps = vec![exact(
        ROOT,
        "RootProj",
        BRANCHES
            .iter()
            .map(|&(id, _, _)| required(id, None))
            .collect(),
    )];
    // Empty listings spend the request budget without spending the node budget.
    steps.extend(
        BRANCHES[..47]
            .iter()
            .map(|&(_, _, path)| Step::ok(path, "[]")),
    );
    let (graph, _) = read_graph(request(), steps).await;
    assert_eq!(rows(&graph, "nodes").len(), 1);
    assert_eq!(graph["complete"], false);
    assert_eq!(graph["truncated"], true);
    assert_eq!(
        edge(&graph, "Root0001", &required("Branch47", None))["state"],
        "no_compatible_version"
    );
    assert_eq!(
        edge(&graph, "Root0001", &required("Branch48", None))["state"],
        "limit_reached"
    );
}
