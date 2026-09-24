//! Tests for the one exec admission (`grants::admit_exec`) and the one
//! human-fence tool list (`operations::HUMAN_FENCED_TOOLS`). Kept apart from
//! `tests.rs`, whose linearization tests stay byte-unchanged.

use super::*;

fn runtime(doc: DiskDoc) -> Runtime {
    let root = std::env::temp_dir();
    Runtime {
        app: None,
        path: root.join("deck-admission-unused.json"),
        socket: root.join("deck-admission-unused.sock"),
        doc: Mutex::new(Ok(doc)),
        io: Mutex::new(()),
        delivery: Mutex::new(()),
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: "svc_test".into(),
        runner_auth: Mutex::new(HashMap::new()),
        started: Instant::now(),
    }
}

fn client() -> Client {
    Client {
        id: "client_a".into(),
        name: "Client A".into(),
        credential_hash: sha(b"mcp_test"),
        credential_version: 1,
        revoked_at: None,
        allow_create: true,
        projects: vec![ProjectScope {
            project_id: "P1".into(),
            roots: vec!["/nonexistent-deck-admission".into()],
        }],
        create_sequence: 0,
    }
}

fn session() -> ManagedSession {
    ManagedSession {
        session_id: "mcp_a".into(),
        card_id: "M1".into(),
        tmux_session: "deck-mcp-admission".into(),
        project_id: "P1".into(),
        title: "MCP shell".into(),
        cwd: "/nonexistent-deck-admission".into(),
        generation: "g_a".into(),
        runner_socket: "/nonexistent-deck-admission/runner.sock".into(),
        owner_client_id: "client_a".into(),
        control_owner: Some("client_a".into()),
        control_holder: Some("holder_a".into()),
        control_epoch: 1,
        lease_expires_at: Some(now_ms() + 60_000),
        human_lock: false,
        output_shared: true,
        closing: false,
        created_at: now_ms(),
        control_sequence: 0,
    }
}

fn grant() -> ExecutionGrant {
    ExecutionGrant {
        grant_id: "grant_a".into(),
        client_id: "client_a".into(),
        credential_version: 1,
        project_id: "P1".into(),
        session_id: "mcp_a".into(),
        session_generation: "g_a".into(),
        profile: "trusted-host-v1".into(),
        environment_profile: ENVIRONMENT_PROFILE.into(),
        environment_profile_version: 1,
        issued_at: now_ms(),
        expires_at: now_ms() + 60_000,
        issued_monotonic_ms: 0,
        duration_ms: 60_000,
        allow_stdin: true,
        allow_output: true,
        _legacy_allow_shell: None,
        grant_version: 1,
        revocation_version: 0,
        service_instance: "svc_test".into(),
        revoked_at: None,
    }
}

fn doc_with(grant: Option<ExecutionGrant>) -> DiskDoc {
    let mut doc = DiskDoc::default();
    doc.config.enabled = true;
    doc.config.clients.push(client());
    doc.sessions.push(session());
    doc.execution_grants.extend(grant);
    doc
}

const EXACT: ControlClaim<'static> = ControlClaim {
    generation: "g_a",
    epoch: Some(1),
    holder_id: Some("holder_a"),
};

/// Admit `session` (as changed by `edit`) under `claim`, against a document
/// that has the grant or not.
fn admit(
    with_grant: bool,
    edit: impl FnOnce(&mut ManagedSession),
    claim: &ControlClaim<'_>,
    stage: ExecStage,
) -> Result<String, AdmissionReason> {
    let runtime = runtime(doc_with(with_grant.then(grant)));
    let mut target = session();
    edit(&mut target);
    runtime
        .read(|doc| {
            admit_exec(&runtime, doc, "client_a", &target, claim, stage)
                .map(|grant| grant.grant_id.clone())
        })
        .unwrap()
}

/// name, grant present, session edit, claim, expected reason
type Case = (
    &'static str,
    bool,
    fn(&mut ManagedSession),
    ControlClaim<'static>,
    AdmissionReason,
);

#[test]
fn admit_exec_names_each_reason_and_returns_the_grant_otherwise() {
    let keep = |_: &mut ManagedSession| {};
    assert_eq!(
        admit(true, keep, &EXACT, ExecStage::Accept),
        Ok("grant_a".into())
    );
    let cases: [Case; 8] = [
        (
            "generation",
            true,
            |_| {},
            ControlClaim {
                generation: "g_b",
                ..EXACT
            },
            AdmissionReason::GenerationChanged,
        ),
        (
            "human",
            true,
            |s| s.human_lock = true,
            EXACT,
            AdmissionReason::HumanControl,
        ),
        (
            "owner",
            true,
            |s| s.control_owner = None,
            EXACT,
            AdmissionReason::ControlRevoked,
        ),
        (
            "holder",
            true,
            |_| {},
            ControlClaim {
                holder_id: Some("holder_b"),
                ..EXACT
            },
            AdmissionReason::HolderMismatch,
        ),
        (
            "epoch",
            true,
            |_| {},
            ControlClaim {
                epoch: Some(2),
                ..EXACT
            },
            AdmissionReason::EpochMismatch,
        ),
        (
            "lease",
            true,
            |s| s.lease_expires_at = Some(now_ms() - 1),
            EXACT,
            AdmissionReason::LeaseExpired,
        ),
        (
            "grant",
            false,
            |_| {},
            EXACT,
            AdmissionReason::GrantRequired,
        ),
        (
            "closing",
            true,
            |s| s.closing = true,
            EXACT,
            AdmissionReason::Closing,
        ),
    ];
    for (name, with_grant, edit, claim, reason) in cases {
        assert_eq!(
            admit(with_grant, edit, &claim, ExecStage::Accept),
            Err(reason),
            "{name}"
        );
    }
}

/// The order is the one `exec` has always used: control before the grant,
/// the grant before `closing`; the final admission never re-checks `closing`
/// and an inspect-style claim (no epoch) skips only the epoch.
#[test]
fn admit_exec_order_and_stages() {
    let human_closing = |s: &mut ManagedSession| {
        s.human_lock = true;
        s.closing = true;
    };
    assert_eq!(
        admit(false, human_closing, &EXACT, ExecStage::Accept),
        Err(AdmissionReason::HumanControl)
    );
    assert_eq!(
        admit(false, |s| s.closing = true, &EXACT, ExecStage::Accept),
        Err(AdmissionReason::GrantRequired)
    );
    assert_eq!(
        admit(true, |s| s.closing = true, &EXACT, ExecStage::Final),
        Ok("grant_a".into())
    );
    let inspect = ControlClaim {
        epoch: None,
        ..EXACT
    };
    assert_eq!(
        admit(true, |s| s.control_epoch = 9, &inspect, ExecStage::Accept),
        Ok("grant_a".into())
    );
    assert_eq!(
        admit(
            true,
            |s| s.control_holder = None,
            &inspect,
            ExecStage::Accept
        ),
        Err(AdmissionReason::HolderMismatch)
    );
}

/// `exec` still returns the errors it returned before the extraction; the
/// five control reasons fold into one `ControlRevoked`, as `check_control`
/// always did.
#[test]
fn admission_errors_and_inspect_words_are_unchanged() {
    use AdmissionReason::*;
    let table = [
        (
            GenerationChanged,
            ErrorKind::ContextChanged,
            "CONTEXT_CHANGED",
        ),
        (HumanControl, ErrorKind::ControlRevoked, "HUMAN_CONTROL"),
        (ControlRevoked, ErrorKind::ControlRevoked, "CONTROL_REVOKED"),
        (HolderMismatch, ErrorKind::ControlRevoked, "HOLDER_MISMATCH"),
        (EpochMismatch, ErrorKind::ControlRevoked, "CONTROL_REVOKED"),
        (
            LeaseExpired,
            ErrorKind::ControlRevoked,
            "CONTROL_LEASE_EXPIRED",
        ),
        (GrantRequired, ErrorKind::Perm, "EXECUTION_GRANT_REQUIRED"),
        (Closing, ErrorKind::Locked, "TARGET_CLOSING"),
    ];
    for (reason, kind, word) in table {
        assert_eq!(reason.error().kind(), kind, "{reason:?}");
        assert_eq!(reason.inspect_code(), word, "{reason:?}");
    }
    // check_control is the control half of the same derivation
    let mut revoked = session();
    revoked.control_owner = None;
    let error = check_control(&revoked, "client_a", "g_a", 1, "holder_a").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ControlRevoked);
}

/// A local takeover refuses `deck_job_interrupt` at `route`, before any
/// journal slot is reserved (it used to take an interrupt-reserve slot and
/// fail later under the delivery lock, with the same code).
#[test]
fn takeover_refuses_interrupt_at_route_without_reserving() {
    let mut doc = doc_with(Some(grant()));
    doc.jobs.push(JobBinding {
        job_id: "job_a".into(),
        client_id: "client_a".into(),
        session_id: "mcp_a".into(),
        session_generation: "g_a".into(),
        request_hash: "a".repeat(64),
        operation_id: "op_a".into(),
        grant_id: "grant_a".into(),
        grant_version: 1,
        allow_output: true,
    });
    let runtime = runtime(doc);
    runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .insert("mcp_a".into());
    let value = route(
        &runtime,
        WireRequest {
            version: CONTROL_PROTOCOL,
            client_id: "client_a".into(),
            credential: "mcp_test".into(),
            tool: "deck_job_interrupt".into(),
            arguments: json!({"request_id":"interrupt_a","job_id":"job_a","session_generation":"g_a","control_epoch":1,"holder_id":"holder_a"}),
        },
    );
    assert_eq!(value["error"]["code"], "HUMAN_CONTROL", "{value}");
    assert!(runtime.read(|doc| doc.operations.is_empty()).unwrap());
}

/// `mcp-fixtures/tools.json` `human_fenced` is the same set as the one list
/// `route` and `emergency_denial` share, and a subset of `mutating` plus
/// `deck_job_read`.
#[test]
fn human_fenced_tools_are_the_fixture() {
    let tools: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/mcp-fixtures/tools.json"
    )))
    .unwrap();
    let listed: std::collections::BTreeSet<&str> = tools["human_fenced"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool.as_str().unwrap())
        .collect();
    let fenced: std::collections::BTreeSet<&str> = HUMAN_FENCED_TOOLS.into_iter().collect();
    assert_eq!(fenced.len(), HUMAN_FENCED_TOOLS.len(), "listed twice");
    assert_eq!(fenced, listed);
    let mutating: Vec<&str> = tools["mutating"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool.as_str().unwrap())
        .collect();
    for tool in &fenced {
        assert!(
            mutating.contains(tool) || *tool == "deck_job_read",
            "{tool}"
        );
    }
}
