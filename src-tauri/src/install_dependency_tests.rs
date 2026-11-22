use super::*;
use DependencyStatus::*;

fn compare(dependency: &ModDependency, records: &[InstallMatch]) -> DependencyCheck {
    let mut index = MatchIndex::new(std::slice::from_ref(dependency));
    for record in records {
        index.observe(record);
    }
    index.compare(dependency)
}

fn record(project: &str, version: &str, status: &str) -> InstallMatch {
    InstallMatch {
        install_id: format!("install_{project}_{version}"),
        project_id: project.into(),
        version_id: version.into(),
        status: status.into(),
    }
}

fn declaration(project: Option<&str>, version: Option<&str>) -> ModDependency {
    ModDependency {
        dependency_type: DependencyType::Required,
        project_id: project.map(str::to_owned),
        version_id: version.map(str::to_owned),
        file_name: None,
    }
}

#[test]
fn exact_and_project_only_matches_are_distinct() {
    let entries = [record("abc", "123", "enabled")];
    assert_eq!(
        compare(&declaration(Some("abc"), Some("123")), &entries).status,
        RecordedExact
    );
    assert_eq!(
        compare(&declaration(None, Some("123")), &entries).status,
        RecordedExact
    );
    assert_eq!(
        compare(&declaration(Some("abc"), None), &entries).status,
        RecordedProject
    );
    assert_eq!(
        compare(&declaration(Some("abc"), Some("124")), &entries).status,
        DifferentVersion
    );
    assert_eq!(
        compare(&declaration(Some("ABC"), None), &entries).status,
        NotRecorded
    );
    assert_eq!(
        compare(&declaration(None, Some("124")), &entries).status,
        NotRecorded
    );
}

#[test]
fn filename_and_embedded_references_are_not_local_matches() {
    let mut dep = declaration(None, None);
    dep.file_name = Some("abc-123.jar".into());
    let entries = [record("abc", "123", "enabled")];
    let check = compare(&dep, &entries);
    assert_eq!(check.status, Unconfirmed);
    assert_eq!(check.reason, Some(UnconfirmedReason::FileReference));
    assert!(check.matches.is_empty());
    dep.dependency_type = DependencyType::Embedded;
    dep.project_id = Some("abc".into());
    let check = compare(&dep, &entries);
    assert_eq!(check.status, Embedded);
    assert!(check.matches.is_empty());
}

#[test]
fn partial_duplicate_and_inconsistent_matches_are_not_positive_matches() {
    let dep = declaration(Some("abc"), Some("123"));
    for (records, reason) in [
        (
            vec![record("abc", "123", "partially_restored")],
            UnconfirmedReason::PartialInstall,
        ),
        (
            vec![
                record("abc", "123", "enabled"),
                record("abc", "124", "enabled"),
            ],
            UnconfirmedReason::AmbiguousRecords,
        ),
        (
            vec![
                record("abc", "123", "enabled"),
                record("def", "123", "enabled"),
            ],
            UnconfirmedReason::InconsistentIds,
        ),
    ] {
        let check = compare(&dep, &records);
        assert_eq!(check.status, Unconfirmed);
        assert_eq!(check.reason, Some(reason));
    }
}

#[test]
fn ambiguous_match_list_is_bounded_without_hiding_total_count() {
    let entries: Vec<_> = (0..30)
        .rev()
        .map(|id| InstallMatch {
            install_id: format!("install{id:02}"),
            ..record("abc", "123", "enabled")
        })
        .collect();
    let check = compare(&declaration(Some("abc"), None), &entries);
    assert_eq!(check.match_count, 30);
    assert_eq!(check.matches.len(), MAX_MATCHES_PER_DECLARATION);
    assert_eq!(check.matches[0].install_id, "install00");
    assert_eq!(check.matches[3].install_id, "install03");
    assert_eq!(check.status, Unconfirmed);
}

#[test]
fn dependency_payload_distinguishes_explicit_null_from_omission() {
    assert!(serde_json::from_str::<CheckRequest>(r#"{"gameId":"game"}"#).is_err());
    let unknown: CheckRequest =
        serde_json::from_str(r#"{"gameId":"game","dependencies":null}"#).unwrap();
    let empty: CheckRequest =
        serde_json::from_str(r#"{"gameId":"game","dependencies":[]}"#).unwrap();
    assert!(unknown.dependencies.is_none());
    assert!(empty.dependencies.unwrap().is_empty());
}

#[test]
fn interrupted_journal_scan_does_not_return_an_absence_claim() {
    let connection = Connection::open_in_memory().unwrap();
    let declarations = [declaration(Some("abc"), None)];
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        scan_checks(&connection, "game", &declarations, None, &cancel),
        Err(AppError::Cancelled)
    ));
}

#[test]
fn desktop_operation_cancel_is_scoped_and_registration_is_released() {
    let registry = DesktopDependencyOperations::default();
    let first_id = Uuid::new_v4();
    let second_id = Uuid::new_v4();
    let first = registry.start(Some(first_id)).unwrap();
    let second = registry.start(Some(second_id)).unwrap();
    assert!(matches!(
        registry.start(Some(first_id)),
        Err(AppError::Conflict(_))
    ));
    assert!(!registry.cancel(Uuid::new_v4()));
    assert!(registry.cancel(first_id));
    assert!(first.cancel.is_cancelled());
    assert!(!second.cancel.is_cancelled());
    drop(first);
    assert!(!registry.cancel(first_id));
    let restarted = registry.start(Some(first_id)).unwrap();
    assert!(!restarted.cancel.is_cancelled());
    let running = restarted.cancel.clone();
    drop(restarted);
    assert!(running.is_cancelled());
}
