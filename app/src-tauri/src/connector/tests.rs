//! Unit tests for the Connector host; moved out of `connector/mod.rs` on 2026-09-23.

use super::*;
use rustls::pki_types::{CertificateDer, ServerName};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

fn test_runtime(tag: &str) -> (Arc<Runtime>, tauri::App<tauri::test::MockRuntime>) {
    let app = tauri::test::mock_app();
    let path = std::env::temp_dir().join(format!("deck-connector-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut doc = DiskDoc::fresh().unwrap();
    doc.config = Config {
        enabled: true,
        address: "127.0.0.1".into(),
        port: 8443,
        interface: None,
    };
    save(&path, &doc).unwrap();
    let r = Arc::new(Runtime {
        app: None,
        path: path.clone(),
        doc: Mutex::new(Ok(doc)),
        pairing: Mutex::new(None),
        lifecycle: Mutex::new(()),
        server_epoch: AtomicU64::new(1),
        running_epoch: AtomicU64::new(1),
    });
    (r, app)
}
fn request(id: &str, text: &str) -> CommandRequest {
    CommandRequest {
        id: id.into(),
        kind: "buffer-add".into(),
        card_id: Some("C1".into()),
        expected_generation: ExpectedGeneration::Missing,
        expected_revision: Some("1".into()),
        payload: json!({"text":text}),
        seq: Some(next_seq()),
    }
}

/// Fresh, increasing phone sequences for test commands.
fn next_seq() -> u64 {
    thread_local! {
        static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
    }
    NEXT.with(|next| {
        let value = next.get();
        next.set(value + 1);
        value
    })
}

fn device(id: &str) -> Device {
    Device {
        id: id.into(),
        name: format!("device-{id}"),
        token_hash: sha(format!("token-{id}").as_bytes()),
        paired_at: 1,
        revoked_at: None,
        history_pruned: false,
        retired_through: 0,
    }
}
fn tombstone(device_id: &str, request: &CommandRequest, state: &str) -> JournalEntry {
    JournalEntry {
        handle: sha(format!("{device_id}\0{}", request.id).as_bytes()),
        device_id: device_id.into(),
        request_hash: sha(&serde_json::to_vec(request).unwrap()),
        id: request.id.clone(),
        kind: request.kind.clone(),
        seq: request.seq,
        request: None,
        state: state.into(),
        code: Some("fixture".into()),
        result: None,
        accepted_at: 1,
        updated_at: 1,
    }
}
fn pending(device_id: &str, request: &CommandRequest, state: &str) -> JournalEntry {
    JournalEntry {
        request: Some(request.clone()),
        code: None,
        ..tombstone(device_id, request, state)
    }
}

#[test]
fn resolved_commands_are_persisted_as_tombstones_that_replay_their_result() {
    let (r, _app) = test_runtime("tombstone-replay");
    r.with_doc(|d| {
        d.devices.push(device("D"));
        Ok(())
    })
    .unwrap();
    let body = request("T1", "secret phone note");
    r.accept(1, "D", body.clone()).unwrap();
    let handle = sha(b"D\0T1");
    r.with_doc(|d| {
        let c = d.commands.iter_mut().find(|c| c.handle == handle).unwrap();
        c.state = "applied".into();
        c.result = Some(json!({"cardId":"C1","entryId":"E1","revision":"2"}));
        Ok(())
    })
    .unwrap();
    let entry = r.read(|d| d.commands[0].clone()).unwrap();
    assert!(entry.request.is_none(), "terminal entry keeps no body");
    let file = std::fs::read_to_string(&r.path).unwrap();
    assert!(!file.contains("secret phone note"));
    assert!(file.contains("\"version\":3"));

    let replay = r.accept(1, "D", body).unwrap();
    assert_eq!(replay.state, "applied");
    assert_eq!(replay.result.as_ref().unwrap()["entryId"], "E1");
    let query = r.command_result("D", "T1").unwrap();
    assert_eq!(query.id, "T1");
    assert_eq!(query.state, "applied");
    assert_eq!(
        r.accept(1, "D", request("T1", "other")).unwrap_err().kind(),
        ErrorKind::ContextChanged
    );
    let reloaded = load(&r.path).unwrap();
    assert!(reloaded.commands[0].request.is_none());
    assert_eq!(reloaded.commands[0].id, "T1");
}

#[test]
fn dropped_tombstones_mark_the_device_so_unknown_ids_are_expired_not_missing() {
    let (r, _app) = test_runtime("tombstone-bound");
    r.with_doc(|d| {
        d.devices.push(device("D"));
        d.devices.push(device("E"));
        d.commands = (0..MAX_TOMBSTONES)
            .map(|i| tombstone("D", &request(&format!("I{i}"), "a"), "rejected"))
            .collect();
        Ok(())
    })
    .unwrap();
    assert_eq!(
        r.command_result("E", "never-sent").unwrap_err().kind(),
        ErrorKind::Missing,
        "a device with complete history may prove absence"
    );
    r.accept(1, "E", request("fresh", "a")).unwrap();
    r.with_doc(|d| {
        d.commands.last_mut().unwrap().state = "rejected".into();
        Ok(())
    })
    .unwrap();
    let doc = r.read(Clone::clone).unwrap();
    assert_eq!(doc.commands.len(), MAX_TOMBSTONES);
    assert!(doc.commands.iter().all(|c| c.id != "I0"));
    assert!(
        doc.devices
            .iter()
            .find(|d| d.id == "D")
            .unwrap()
            .history_pruned
    );
    assert!(
        !doc.devices
            .iter()
            .find(|d| d.id == "E")
            .unwrap()
            .history_pruned
    );
    let expired = r.command_result("D", "I0").unwrap_err();
    assert_eq!(expired.kind(), ErrorKind::ContextChanged);
    assert_eq!(expired.message(), COMMAND_EXPIRED);
    assert_eq!(r.command_result("D", "I1").unwrap().state, "rejected");
    assert_eq!(load(&r.path).unwrap().commands.len(), MAX_TOMBSTONES);
}

#[test]
fn admission_encodes_the_document_once() {
    let (r, _app) = test_runtime("single-encode");
    r.with_doc(|d| {
        d.devices.push(device("D"));
        Ok(())
    })
    .unwrap();
    ENCODES.with(|count| count.set(0));
    r.accept(1, "D", request("once", "a")).unwrap();
    assert_eq!(ENCODES.with(std::cell::Cell::get), 1);
}

#[test]
fn v1_state_loads_compacted_and_newer_versions_are_refused_untouched() {
    let (r, _app) = test_runtime("v1-migration");
    let done = request("done", "old body");
    let open = request("open", "queued body");
    let mut doc = r.read(Clone::clone).unwrap();
    doc.devices.push(device("D"));
    doc.commands = vec![
        JournalEntry {
            state: "applied".into(),
            code: None,
            result: Some(json!({"cardId":"C1","entryId":"E1","revision":"2"})),
            ..pending("D", &done, "applied")
        },
        pending("D", &open, "accepted"),
    ];
    let mut v1 = serde_json::to_value(&doc).unwrap();
    v1["version"] = json!(1);
    for entry in v1["commands"].as_array_mut().unwrap() {
        let entry = entry.as_object_mut().unwrap();
        entry.remove("id");
        entry.remove("kind");
    }
    std::fs::write(&r.path, serde_json::to_vec(&v1).unwrap()).unwrap();
    let loaded = load(&r.path).unwrap();
    assert_eq!(loaded.version, VERSION);
    assert_eq!(loaded.commands[0].id, "done");
    assert!(loaded.commands[0].request.is_none());
    assert_eq!(loaded.commands[1].request.as_ref(), Some(&open));

    // A v1 entry without its request is not a v1 file.
    let mut broken = v1.clone();
    broken["commands"][1]
        .as_object_mut()
        .unwrap()
        .remove("request");
    std::fs::write(&r.path, serde_json::to_vec(&broken).unwrap()).unwrap();
    assert_eq!(load(&r.path).err().unwrap().kind(), ErrorKind::Recovery);

    let mut future = v1;
    future["version"] = json!(VERSION + 1);
    let bytes = serde_json::to_vec(&future).unwrap();
    std::fs::write(&r.path, &bytes).unwrap();
    assert_eq!(load(&r.path).err().unwrap().kind(), ErrorKind::Recovery);
    assert_eq!(std::fs::read(&r.path).unwrap(), bytes, "refused untouched");
}

#[test]
fn revocation_drops_history_and_frees_device_capacity_only_when_idle() {
    let mut doc = DiskDoc::fresh().unwrap();
    doc.devices = (0..MAX_DEVICES).map(|i| device(&format!("D{i}"))).collect();
    doc.commands = vec![
        tombstone("D0", &request("old", "a"), "rejected"),
        pending("D0", &request("queued", "a"), "accepted"),
        pending("D0", &request("running", "a"), "executing"),
        tombstone("D1", &request("kept", "a"), "rejected"),
    ];
    revoke_device(&mut doc, "D0").unwrap();
    let d0 = doc
        .commands
        .iter()
        .filter(|c| c.device_id == "D0")
        .collect::<Vec<_>>();
    assert_eq!(d0.len(), 1, "only the in-flight entry survives");
    assert_eq!(d0[0].id, "running");
    assert_eq!(d0[0].state, "ambiguous");
    assert!(doc.commands.iter().any(|c| c.id == "kept"));

    prune_revoked_devices(&mut doc);
    assert_eq!(
        doc.devices.len(),
        MAX_DEVICES - 1,
        "an idle revoked device is freed"
    );
    assert!(doc.commands.iter().all(|c| c.device_id != "D0"));

    // A revoked device that still has unresolved work keeps its slot.
    doc.devices.push(device("D0"));
    doc.devices[0].revoked_at = Some(1);
    let busy = doc.devices[0].id.clone();
    doc.commands
        .push(pending(&busy, &request("busy", "a"), "executing"));
    prune_revoked_devices(&mut doc);
    assert_eq!(doc.devices.len(), MAX_DEVICES);
    assert!(doc.devices.iter().any(|d| d.id == busy));
}

#[test]
fn pairing_reuses_a_revoked_device_slot() {
    let (r, _app) = test_runtime("device-capacity");
    r.with_doc(|d| {
        d.devices = (0..MAX_DEVICES).map(|i| device(&format!("D{i}"))).collect();
        Ok(())
    })
    .unwrap();
    let arm = |r: &Runtime| {
        *r.pairing.lock_or_recover() = Some(Pairing {
            code: "code".into(),
            expires_at: now() + 30,
        });
    };
    arm(&r);
    assert_eq!(
        r.pair(1, "code", "full").unwrap_err().kind(),
        ErrorKind::DiskFull
    );
    r.with_doc(|d| revoke_device(d, "D3")).unwrap();
    arm(&r);
    let paired = r.pair(1, "code", "phone 33").unwrap();
    let devices = r.read(|d| d.devices.clone()).unwrap();
    assert_eq!(devices.len(), MAX_DEVICES);
    assert!(devices.iter().all(|d| d.id != "D3"));
    assert!(devices.iter().any(|d| d.id == paired["deviceId"]));
}

struct FakeOutput {
    agent: bool,
    mcp_owned: bool,
    pane_calls: std::cell::Cell<usize>,
}
impl OutputIo for FakeOutput {
    fn card(&self, id: &str) -> Result<InternalCard, DeckError> {
        Ok(InternalCard {
            id: id.into(),
            session: "deck-card-0001".into(),
            agent_target: self.agent,
        })
    }
    fn probe(&self, _: &str) -> Result<crate::context::ConnectorProbe, DeckError> {
        self.pane_calls.set(self.pane_calls.get() + 1);
        Err(DeckError::new(ErrorKind::NoSession, "fixture"))
    }
    fn tmux(&self, _: &[String]) -> Result<String, DeckError> {
        self.pane_calls.set(self.pane_calls.get() + 1);
        Err(DeckError::new(ErrorKind::Tmux, "fixture"))
    }
    fn mcp_fence(&self, _: &str) -> Result<(), DeckError> {
        if self.mcp_owned {
            Err(DeckError::new(ErrorKind::Perm, "MCP owns terminal control"))
        } else {
            Ok(())
        }
    }
}

struct SavedAgentWithForeground {
    foreground_agents: Vec<Option<String>>,
    probe_calls: std::cell::Cell<usize>,
    pane_calls: std::cell::Cell<usize>,
}
impl OutputIo for SavedAgentWithForeground {
    fn card(&self, id: &str) -> Result<InternalCard, DeckError> {
        Ok(InternalCard {
            id: id.into(),
            session: "deck-card-0001".into(),
            agent_target: true,
        })
    }
    fn probe(&self, _: &str) -> Result<crate::context::ConnectorProbe, DeckError> {
        let index = self.probe_calls.get();
        self.probe_calls.set(index + 1);
        Ok(crate::context::ConnectorProbe {
            identity: crate::context::PaneIdentity {
                server_pid: 1,
                session_id: "$1".into(),
                window_id: "@1".into(),
                pane_id: "%1".into(),
                pane_pid: 2,
            },
            agent: self
                .foreground_agents
                .get(index)
                .or_else(|| self.foreground_agents.last())
                .cloned()
                .flatten(),
            foreground_pid: 3,
            start_seconds: 4,
            start_micros: 5,
            generation: "generation".into(),
        })
    }
    fn tmux(&self, args: &[String]) -> Result<String, DeckError> {
        self.pane_calls.set(self.pane_calls.get() + 1);
        Ok(
            if args.first().is_some_and(|arg| arg == "display-message") {
                "0"
            } else {
                "secret"
            }
            .into(),
        )
    }
    fn mcp_fence(&self, _: &str) -> Result<(), DeckError> {
        Ok(())
    }
}

#[test]
fn phone_output_and_send_are_limited_to_saved_agent_cards() {
    let shell = FakeOutput {
        agent: false,
        mcp_owned: false,
        pane_calls: std::cell::Cell::new(0),
    };
    let refused = output_with(&shell, "C1").unwrap_err();
    assert_eq!(refused.kind(), ErrorKind::Invalid);
    assert_eq!(refused.message(), "unsupported-target");
    assert_eq!(shell.pane_calls.get(), 0, "no pane is touched");

    let agent = FakeOutput {
        agent: true,
        mcp_owned: false,
        pane_calls: std::cell::Cell::new(0),
    };
    assert!(output_with(&agent, "C1").is_err());
    assert_eq!(agent.pane_calls.get(), 1, "an agent card reaches the probe");

    let mcp = FakeOutput {
        agent: true,
        mcp_owned: true,
        pane_calls: std::cell::Cell::new(0),
    };
    let refused = output_with(&mcp, "C1").unwrap_err();
    assert_eq!(refused.message(), "unsupported-target");
    assert_eq!(
        mcp.pane_calls.get(),
        0,
        "an MCP-controlled pane is not touched"
    );

    let board = json!({"cards":[
        {"id":"S","session":"deck-s-0001","cmd":""},
        {"id":"Z","session":"deck-z-0001","cmd":"/bin/zsh"},
        {"id":"A","session":"deck-a-0001","cmd":"claude"},
    ]});
    assert!(!card_in(&board, "S").unwrap().agent_target);
    assert!(!card_in(&board, "Z").unwrap().agent_target);
    assert!(card_in(&board, "A").unwrap().agent_target);
}

#[test]
fn phone_output_refuses_a_saved_agent_card_when_foreground_is_shell() {
    let shell = SavedAgentWithForeground {
        foreground_agents: vec![None],
        probe_calls: std::cell::Cell::new(0),
        pane_calls: std::cell::Cell::new(0),
    };
    let refused = output_with(&shell, "C1").unwrap_err();
    assert_eq!(refused.kind(), ErrorKind::Invalid);
    assert_eq!(refused.message(), "unsupported-target");
    assert_eq!(shell.pane_calls.get(), 0, "no output is captured");
}

#[test]
fn phone_output_drops_captured_bytes_when_agent_returns_to_shell() {
    let changed = SavedAgentWithForeground {
        foreground_agents: vec![Some("claude".into()), None],
        probe_calls: std::cell::Cell::new(0),
        pane_calls: std::cell::Cell::new(0),
    };
    let refused = output_with(&changed, "C1").unwrap_err();
    assert_eq!(refused.kind(), ErrorKind::Invalid);
    assert_eq!(refused.message(), "unsupported-target");
    assert_eq!(
        changed.pane_calls.get(),
        2,
        "capture happened before recheck"
    );
}

#[test]
fn restart_listens_only_on_the_recorded_interface() {
    let lan: Ipv4Addr = "192.168.1.20".parse().unwrap();
    let config = |interface: Option<&str>| Config {
        enabled: true,
        address: lan.to_string(),
        port: 47631,
        interface: interface.map(str::to_owned),
    };
    let at = |interface: &str, netmask: [u8; 4]| LocalAddress {
        ip: lan,
        interface: interface.into(),
        netmask: netmask.into(),
    };
    let here = vec![at("en0", [255, 255, 255, 0])];
    let elsewhere = vec![at("en7", [255, 255, 255, 0])];
    let started = listener_network_ok(&config(Some("en0")), &here).unwrap();
    assert_eq!(started, here[0]);
    // Same address and interface on a differently sized subnet is a
    // different network for the running listener's recheck.
    let resized = vec![at("en0", [255, 255, 0, 0])];
    assert_ne!(
        listener_network_ok(&config(Some("en0")), &resized).unwrap(),
        started
    );
    // Both interfaces carry it: the recorded one is chosen.
    let both = vec![elsewhere[0].clone(), here[0].clone()];
    assert_eq!(
        listener_network_ok(&config(Some("en0")), &both).unwrap(),
        started
    );
    assert_eq!(
        listener_network_ok(&config(Some("en0")), &elsewhere)
            .unwrap_err()
            .kind(),
        ErrorKind::ContextChanged
    );
    assert!(listener_network_ok(&config(None), &elsewhere).is_ok());
    assert_eq!(
        listener_network_ok(&config(Some("en0")), &[])
            .unwrap_err()
            .kind(),
        ErrorKind::Invalid
    );
    assert_eq!(interface_of(lan, &here).as_deref(), Some("en0"));
}

#[test]
fn output_bounds_history_and_utf8_tail_independently() {
    let short = "trust line\nlatest marker".to_string();
    assert_eq!(bounded_output(short.clone(), 201), (short, true));

    let boundary = "a".repeat(MAX_OUTPUT_BYTES);
    assert_eq!(bounded_output(boundary.clone(), 200), (boundary, false));

    let crossing = format!("é{}", "a".repeat(MAX_OUTPUT_BYTES - 1));
    let expected_tail = "a".repeat(MAX_OUTPUT_BYTES - 1);
    assert_eq!(bounded_output(crossing, 200), (expected_tail, true));
}

#[test]
fn generation_null_asserts_stopped_and_missing_is_rejected_for_queue_commands() {
    let raw = json!({
        "id":"pause-1",
        "kind":"queue-pause",
        "cardId":"C1",
        "expectedGeneration":null,
        "payload":{"itemId":"Q1","paused":true,"revision":"1"}
    });
    let stopped: CommandRequest = serde_json::from_value(raw).unwrap();
    assert_eq!(stopped.expected_generation, ExpectedGeneration::Stopped);
    assert!(validate_command(&stopped).is_ok());

    let mut missing = stopped;
    missing.expected_generation = ExpectedGeneration::Missing;
    assert_eq!(
        validate_command(&missing).unwrap_err().kind(),
        ErrorKind::Invalid
    );

    let mut unknown = missing;
    unknown.expected_generation = ExpectedGeneration::Stopped;
    unknown.payload["unexpected"] = json!(true);
    assert_eq!(
        validate_command(&unknown).unwrap_err().kind(),
        ErrorKind::Invalid
    );
}

#[test]
fn every_wire_command_has_a_closed_valid_and_invalid_payload_contract() {
    let make = |kind: &str, payload: Value| CommandRequest {
        id: format!("{kind}-1"),
        kind: kind.into(),
        card_id: (kind != "task-create").then(|| "C1".into()),
        expected_generation: if kind == "send-message" {
            ExpectedGeneration::Live("a".repeat(64))
        } else if matches!(kind, "queue-pause" | "queue-cancel") {
            ExpectedGeneration::Stopped
        } else {
            ExpectedGeneration::Missing
        },
        expected_revision: matches!(
            kind,
            "buffer-add" | "buffer-edit" | "buffer-delete" | "buffer-queue" | "task-create"
        )
        .then(|| "7".into()),
        payload,
        seq: Some(1),
    };

    let valid = [
        make("send-message", json!({"text":"hello\nworld"})),
        make("buffer-add", json!({"text":"note"})),
        make("buffer-edit", json!({"entryId":"E1","text":"replacement"})),
        make("buffer-delete", json!({"entryId":"E1"})),
        make("buffer-queue", json!({"entryIds":["E1","E2"]})),
        make(
            "task-create",
            json!({"projectId":"P1","presetId":"preset-1"}),
        ),
        make(
            "queue-pause",
            json!({"itemId":"Q1","paused":true,"revision":"12"}),
        ),
        make("queue-cancel", json!({"itemId":"Q1","revision":"12"})),
    ];
    for request in &valid {
        assert!(validate_command(request).is_ok(), "{}", request.kind);
    }

    let invalid = [
        make("send-message", json!({"text":""})),
        make("send-message", json!({"text":"bad\u{0}text"})),
        make("buffer-add", json!({"text":"ok","extra":true})),
        make("buffer-edit", json!({"entryId":"","text":"ok"})),
        make("buffer-delete", json!({"entryId":"bad\nidentity"})),
        make("buffer-queue", json!({"entryIds":[]})),
        make("buffer-queue", json!({"entryIds":["E1","E1"]})),
        make("task-create", json!({"projectId":"P1","presetId":""})),
        make("queue-pause", json!({"itemId":"Q1","revision":"12"})),
        make(
            "queue-cancel",
            json!({"itemId":"Q1","paused":false,"revision":"12"}),
        ),
        make("queue-cancel", json!({"itemId":"Q1","revision":"v12"})),
    ];
    for request in &invalid {
        assert_eq!(
            validate_command(request).unwrap_err().kind(),
            ErrorKind::Invalid,
            "{}",
            request.kind
        );
    }

    let mut malformed = valid[0].clone();
    malformed.id = "bad\nidentity".into();
    assert_eq!(
        validate_command(&malformed).unwrap_err().kind(),
        ErrorKind::Invalid
    );
    malformed = valid[0].clone();
    malformed.kind = "shell-command".into();
    assert_eq!(
        validate_command(&malformed).unwrap_err().kind(),
        ErrorKind::Invalid
    );
    malformed = valid[0].clone();
    malformed.card_id = None;
    assert_eq!(
        validate_command(&malformed).unwrap_err().kind(),
        ErrorKind::Invalid
    );
    malformed = valid[0].clone();
    malformed.expected_generation = ExpectedGeneration::Stopped;
    assert_eq!(
        validate_command(&malformed).unwrap_err().kind(),
        ErrorKind::Invalid
    );
    malformed = valid[1].clone();
    malformed.expected_revision = None;
    assert_eq!(
        validate_command(&malformed).unwrap_err().kind(),
        ErrorKind::Invalid
    );

    assert!(command_id("literal-id"));
    assert!(!command_id(""));
    assert!(!command_id("bad\tid"));
    assert!(command_text("tabs\tand\nlines"));
    assert!(!command_text(""));
    assert_eq!(external_state("executing"), "accepted");
    assert_eq!(external_state("applied"), "applied");
}

#[test]
fn command_surface_preserves_the_durable_lifecycle_and_closes_on_disable() {
    use crate::prompt_delivery::Transport;

    let (runtime, _app) = test_runtime("command-surface");
    assert!(RUNTIME.set(runtime.clone()).is_ok());
    runtime
        .with_doc(|doc| {
            for id in ["D1", "D2"] {
                doc.devices.push(Device {
                    id: id.into(),
                    name: format!("device-{id}"),
                    token_hash: sha(format!("token-{id}").as_bytes()),
                    paired_at: 1,
                    revoked_at: None,
                    history_pruned: false,
                    retired_through: 0,
                });
            }
            Ok(())
        })
        .unwrap();

    let accepted = runtime.accept(1, "D1", request("surface", "note")).unwrap();
    assert_eq!(accepted.state, "accepted");
    assert_eq!(
        serde_json::to_value(&accepted).unwrap()["state"],
        "accepted"
    );
    let handle = sha(b"D1\0surface");
    let status = connector_status().unwrap();
    assert!(status.enabled);
    assert!(status.running);
    assert_eq!(status.devices.len(), 2);
    assert!(status.origin.as_deref().unwrap().starts_with("https://"));
    let status_wire = serde_json::to_value(&status).unwrap();
    assert_eq!(status_wire["devices"].as_array().unwrap().len(), 2);
    assert_eq!(status_wire["enabled"], true);
    assert_eq!(connector_pending().unwrap().len(), 1);

    let claimed = connector_claim(handle.clone()).unwrap();
    assert_eq!(claimed.request.id, "surface");
    assert_eq!(
        serde_json::to_value(&claimed).unwrap()["request"]["id"],
        "surface"
    );
    assert!(connector_pending().unwrap().is_empty());
    assert_eq!(
        connector_claim(handle.clone()).err().unwrap().kind(),
        ErrorKind::Other
    );
    assert_eq!(
        connector_complete(handle.clone(), "unknown".into(), None, None)
            .unwrap_err()
            .kind(),
        ErrorKind::Invalid
    );
    connector_complete(
        handle.clone(),
        "applied".into(),
        None,
        Some(json!({"cardId":"C1","entryId":"E1","revision":"2"})),
    )
    .unwrap();
    assert_eq!(
        connector_complete(handle.clone(), "applied".into(), None, None)
            .unwrap_err()
            .kind(),
        ErrorKind::Other
    );
    assert_eq!(
        connector_validate(handle).unwrap_err().kind(),
        ErrorKind::ContextChanged
    );

    connector_revoke("D2".into()).unwrap();
    assert_eq!(
        connector_revoke("missing".into()).unwrap_err().kind(),
        ErrorKind::Missing
    );
    connector_disable().unwrap();
    let disabled = connector_status().unwrap();
    assert!(!disabled.enabled);
    assert!(!disabled.running);
    assert!(disabled.origin.is_none());
    assert_eq!(runtime.read(|doc| doc.version).unwrap(), VERSION);
    assert!(runtime
        .read(|doc| doc.host_id.starts_with("host_"))
        .unwrap());
    assert_eq!(runtime.read(|doc| doc.devices.len()).unwrap(), 2usize);
    assert_eq!(runtime.read(|doc| doc.commands.len()).unwrap(), 1usize);
    assert!(!runtime.read(|doc| doc.config.clone()).unwrap().enabled);
    assert_eq!(
        runtime.read(|doc| doc.identity_address.clone()).unwrap(),
        None
    );
    assert_eq!(
        runtime
            .read(|doc| doc.identity_fingerprint.clone())
            .unwrap(),
        None
    );
    assert_eq!(
        runtime
            .read(|doc| (doc.config.address.clone(), doc.config.port))
            .unwrap(),
        ("127.0.0.1".into(), 8443)
    );
    assert_eq!(
        runtime
            .read(|doc| doc
                .devices
                .iter()
                .filter(|device| device.revoked_at.is_some())
                .count())
            .unwrap(),
        1
    );
    assert_eq!(
        runtime
            .read(|doc| doc.commands.first().map(|command| command.state.clone()))
            .unwrap()
            .as_deref(),
        Some("applied")
    );
    assert_eq!(
        runtime
            .read(|doc| {
                doc.devices
                    .iter()
                    .map(|device| device.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap(),
        vec!["device-D1".to_string(), "device-D2".to_string()]
    );
    assert_eq!(
        runtime
            .read(|doc| {
                doc.devices
                    .iter()
                    .map(|device| device.id.clone())
                    .collect::<HashSet<_>>()
            })
            .unwrap(),
        HashSet::from(["D1".to_string(), "D2".to_string()])
    );
    assert_eq!(
        runtime
            .read(|doc| json!({
                "enabled": doc.config.enabled,
                "commands": doc.commands.len(),
                "devices": doc.devices.len()
            }))
            .unwrap(),
        json!({"enabled":false,"commands":1,"devices":2})
    );
    assert_eq!(
        connector_claim("missing".into()).err().unwrap().kind(),
        ErrorKind::Perm
    );
    assert_eq!(connector_pairing().err().unwrap().kind(), ErrorKind::Other);
    assert_eq!(
        connector_validate_admission("invalid".into())
            .unwrap_err()
            .kind(),
        ErrorKind::Invalid
    );
    assert_eq!(
        connector_smoke_seed("C1".into(), "1".into())
            .err()
            .unwrap()
            .kind(),
        ErrorKind::Other
    );
    assert_eq!(
        connector_smoke_transport("C1".into()).err().unwrap().kind(),
        ErrorKind::Other
    );
    assert_eq!(
        serde_json::to_value(SmokeTransportView {
            path: "/private/fixture".into()
        })
        .unwrap()["path"],
        "/private/fixture"
    );
    let pairing_wire = serde_json::to_value(PairingView {
        uri: "deck-connector://pair?data=fixture".into(),
        svg: "<svg/>".into(),
        expires_at: 42,
        origin: "https://192.168.1.2:8443".into(),
        fingerprint: "a".repeat(64),
    })
    .unwrap();
    assert_eq!(pairing_wire["expiresAt"], 42);
    let invalid_enable = tauri::async_runtime::block_on(connector_enable("public".into(), 80));
    assert_eq!(invalid_enable.err().unwrap().kind(), ErrorKind::Invalid);

    let queues = Queues::new(crate::scheduler::QueueState::default());
    assert!(crate::scheduler::claim_session(&queues.busy, "busy"));
    {
        let _claim = BusyClaim {
            busy: &queues.busy,
            session: "busy",
        };
        assert!(queues.busy.lock_or_recover().contains("busy"));
    }
    assert!(!queues.busy.lock_or_recover().contains("busy"));

    let transport = ConnectorTransport {
        card_id: "C1",
        session: "S1",
        expected_generation: "generation",
        device_id: "D1",
    };
    assert_eq!(transport.guard().unwrap_err().kind(), ErrorKind::Perm);
    assert_eq!(
        transport
            .run(&["display-message".into()])
            .unwrap_err()
            .kind(),
        ErrorKind::Perm
    );
    assert_eq!(
        transport
            .run_with_stdin(&["load-buffer".into()], b"literal")
            .unwrap_err()
            .kind(),
        ErrorKind::Perm
    );
    transport.pause(std::time::Duration::ZERO);

    for kind in ["send-message", "queue-pause", "queue-cancel"] {
        let malformed = CommandRequest {
            id: "native-invalid".into(),
            kind: kind.into(),
            card_id: Some("C1".into()),
            expected_generation: ExpectedGeneration::Stopped,
            expected_revision: None,
            payload: json!({"unexpected":true}),
            seq: None,
        };
        assert_eq!(
            execute_native(&malformed, &"a".repeat(64), "D1", &queues),
            Err(("rejected", "invalid-payload"))
        );
    }
    let frontend_command = request("frontend", "note");
    assert_eq!(
        execute_native(&frontend_command, &"a".repeat(64), "D1", &queues),
        Err(("rejected", "frontend-required"))
    );

    assert!(!connector_addresses().iter().any(|address| {
        address
            .parse::<Ipv4Addr>()
            .is_ok_and(|ip| !connector_network_range(ip))
    }));
    assert!(!host_name().chars().any(char::is_control));
}

#[test]
fn unknown_probe_is_not_reported_as_stopped() {
    assert_eq!(
        probe_status::<()>(Err(DeckError::new(ErrorKind::NoSession, "missing"))).0,
        "stopped"
    );
    assert_eq!(
        probe_status::<()>(Err(DeckError::new(ErrorKind::Tmux, "unknown"))).0,
        "unknown"
    );
    assert_eq!(probe_status(Ok(())).0, "running");
}

#[test]
fn terminal_results_are_closed_and_kind_specific() {
    let request = request("result", "text");
    assert!(validate_terminal(
        &request.kind,
        "applied",
        None,
        Some(&json!({"cardId":"C1","entryId":"E1","revision":"2"}))
    ));
    assert!(!validate_terminal(
        &request.kind,
        "applied",
        None,
        Some(&json!({"cardId":"C1","entryId":"E1","revision":"2","extra":true}))
    ));
    assert!(!validate_terminal(
        &request.kind,
        "rejected",
        Some("UPPER_CASE"),
        None
    ));
    assert!(!validate_terminal(
        &request.kind,
        "applied",
        None,
        Some(&json!({"cardId":"C1","entryId":"E1","revision":"x".repeat(129)}))
    ));
}

#[test]
fn escaped_32k_text_roundtrips_but_larger_text_is_rejected_before_acceptance() {
    let (runtime, _app) = test_runtime("text-cap");
    runtime
        .with_doc(|doc| {
            doc.devices.push(Device {
                id: "D".into(),
                name: "device".into(),
                token_hash: "a".repeat(64),
                paired_at: 1,
                revoked_at: None,
                history_pruned: false,
                retired_through: 0,
            });
            Ok(())
        })
        .unwrap();
    let escaped = "\n".repeat(MAX_TEXT);
    let accepted = runtime.accept(1, "D", request("exact", &escaped)).unwrap();
    assert_eq!(accepted.state, "accepted");
    assert_eq!(
        runtime
            .read(
                |doc| doc.commands[0].request.as_ref().unwrap().payload["text"]
                    .as_str()
                    .unwrap()
                    .len()
            )
            .unwrap(),
        MAX_TEXT
    );
    assert!(runtime
        .accept(1, "D", request("too-large", &"x".repeat(MAX_TEXT + 1)))
        .is_err());
    assert_eq!(runtime.read(|doc| doc.commands.len()).unwrap(), 1);
}

#[test]
fn save_and_load_share_the_same_byte_cap_and_failed_growth_does_not_commit() {
    let (runtime, _app) = test_runtime("state-cap");
    let original_host = runtime.read(|doc| doc.host_id.clone()).unwrap();
    assert_eq!(
        runtime
            .with_doc(|doc| {
                doc.host_id = "x".repeat(MAX_STATE_BYTES);
                Ok(())
            })
            .unwrap_err()
            .kind(),
        ErrorKind::DiskFull
    );
    assert_eq!(
        runtime.read(|doc| doc.host_id.clone()).unwrap(),
        original_host
    );
    assert!(std::fs::metadata(&runtime.path).unwrap().len() <= MAX_STATE_BYTES as u64);

    std::fs::write(&runtime.path, vec![b'x'; MAX_STATE_BYTES + 1]).unwrap();
    assert_eq!(
        load(&runtime.path).err().unwrap().kind(),
        ErrorKind::Recovery
    );
}

#[test]
fn admission_reserves_enough_space_for_terminal_result_at_byte_capacity() {
    let (runtime, _app) = test_runtime("terminal-reserve");
    let mut doc = runtime.read(Clone::clone).unwrap();
    doc.devices.push(Device {
        id: "D".into(),
        name: "device".into(),
        token_hash: "a".repeat(64),
        paired_at: 1,
        revoked_at: None,
        history_pruned: false,
        retired_through: 0,
    });
    let make = |index: usize| {
        let request = request(&format!("I{index}"), &"x".repeat(MAX_TEXT));
        JournalEntry {
            handle: sha(format!("D\0{}", request.id).as_bytes()),
            device_id: "D".into(),
            request_hash: sha(&serde_json::to_vec(&request).unwrap()),
            id: request.id.clone(),
            kind: request.kind.clone(),
            seq: request.seq,
            request: Some(request),
            state: "accepted".into(),
            code: None,
            result: None,
            accepted_at: 1,
            updated_at: 1,
        }
    };
    let sample_bytes = serde_json::to_vec(&make(0)).unwrap().len();
    let base_bytes = serde_json::to_vec(&doc).unwrap().len();
    let estimate = (MAX_STATE_BYTES - base_bytes) / (sample_bytes + TERMINAL_RESERVE_BYTES);
    doc.commands = (0..estimate.saturating_sub(2)).map(&make).collect();
    let mut next = doc.commands.len();
    while next < MAX_COMMANDS {
        doc.commands.push(make(next));
        if ensure_admission_budget(&doc).is_err() {
            doc.commands.pop();
            break;
        }
        next += 1;
    }
    assert!(!doc.commands.is_empty());
    let mut over = doc.clone();
    over.commands.push(make(next + 1));
    assert_eq!(
        ensure_admission_budget(&over).unwrap_err().kind(),
        ErrorKind::DiskFull
    );

    let terminal = doc.commands.last_mut().unwrap();
    terminal.state = "applied".into();
    terminal.result = Some(json!({
        "cardId":"C1",
        "entryId":"E1",
        "revision":"9"
    }));
    assert!(validate_terminal(
        &terminal.kind,
        &terminal.state,
        None,
        terminal.result.as_ref()
    ));
    // Every committed write compacts; the terminal result always fits.
    compact(&mut doc);
    save(&runtime.path, &doc).unwrap();
    let reloaded = load(&runtime.path).unwrap();
    assert_eq!(reloaded.commands.last().unwrap().state, "applied");
}

#[test]
fn snapshot_queue_wire_shape_is_a_flat_closed_dto_array() {
    let queue = vec![crate::scheduler::connector::QueueDto {
        id: "Q1".into(),
        card_id: "C1".into(),
        mode: "once".into(),
        state: "pending".into(),
        paused: false,
        revision: "7".into(),
    }];
    let fixture = json!({"queue": queue});
    assert_eq!(
        serde_json::to_vec(&fixture).unwrap(),
        br#"{"queue":[{"cardId":"C1","id":"Q1","mode":"once","paused":false,"revision":"7","state":"pending"}]}"#
    );
}

#[test]
fn admission_proves_handle_derived_durable_copies_without_original_revision() {
    let handle = "a".repeat(64);
    let request = CommandRequest {
        id: "queue-1".into(),
        kind: "buffer-queue".into(),
        card_id: Some("C1".into()),
        expected_generation: ExpectedGeneration::Missing,
        expected_revision: Some("1".into()),
        payload: json!({"entryIds":["E1","E2"]}),
        seq: None,
    };
    let copy1 = buffer_operation_id(&handle, "E1");
    let copy2 = buffer_operation_id(&handle, "E2");
    let board = json!({"cards":[{"id":"C1","cmd":"codex","buffer":{"revision":9,"entries":[
        {"id":"E1","text":"frozen one","copies":[{"operationId":copy1}]},
        {"id":"E2","text":"frozen two","copies":[{"operationId":copy2}]},
        {"id":"manual-later","text":"allowed","copies":[]}
    ]}}]});
    assert!(validate_admission_board(&handle, &request, &board).is_ok());
    assert!(validate_admission_board(&"b".repeat(64), &request, &board).is_err());
    let mut missing = board.clone();
    missing["cards"][0]["buffer"]["entries"][1]["copies"] = json!([]);
    assert!(validate_admission_board(&handle, &request, &missing).is_err());
    missing["cards"][0]["buffer"]["entries"]
        .as_array_mut()
        .unwrap()
        .remove(1);
    assert!(validate_admission_board(&handle, &request, &missing).is_err());
}

#[test]
fn every_card_route_requires_a_saved_trusted_agent_command() {
    assert!(queue_target_supported(&json!({"cmd":"codex"})));
    assert!(queue_target_supported(&json!({"cmd":"claude"})));
    assert!(queue_target_supported(&json!({"cmd":"codex --full-auto"})));
    assert!(queue_target_supported(
        &json!({"cmd":"claude --dangerously-skip-permissions"})
    ));
    for card in [
        json!({"cmd":""}),
        json!({"cmd":"/bin/zsh"}),
        json!({"cmd":"/bin/zsh -lc codex"}),
        json!({"cmd":"codex;zsh"}),
        json!({"cmd":"claude && sh"}),
        json!({"cmd":"env FOO=1 /opt/bin/claude --x"}),
    ] {
        let error = require_queue_target(&card).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Invalid);
        assert_eq!(error.message(), "unsupported-target");
        let internal = InternalCard {
            id: "C1".into(),
            session: "deck-c1-0001".into(),
            agent_target: false,
        };
        assert_eq!(
            require_agent_card(&internal).unwrap_err().message(),
            "unsupported-target"
        );
        for kind in ["buffer-add", "buffer-edit", "buffer-delete", "buffer-queue"] {
            let request = CommandRequest {
                id: format!("{kind}-guard"),
                kind: kind.into(),
                card_id: Some("C1".into()),
                expected_generation: ExpectedGeneration::Missing,
                expected_revision: Some("0".into()),
                payload: Value::Null,
                seq: None,
            };
            assert_eq!(
                validate_buffer_target(&request, &card)
                    .unwrap_err()
                    .message(),
                "unsupported-target"
            );
        }
    }

    let handle = "a".repeat(64);
    let request = CommandRequest {
        id: "queue-guard".into(),
        kind: "buffer-queue".into(),
        card_id: Some("C1".into()),
        expected_generation: ExpectedGeneration::Missing,
        expected_revision: Some("1".into()),
        payload: json!({"entryIds":["E1"]}),
        seq: None,
    };
    let operation_id = buffer_operation_id(&handle, "E1");
    for cmd in ["", "/bin/zsh"] {
        let board = json!({"cards":[{"id":"C1","cmd":cmd,"buffer":{"entries":[{
            "id":"E1","copies":[{"operationId":operation_id}]
        }]}}]});
        assert_eq!(
            validate_admission_board(&handle, &request, &board)
                .unwrap_err()
                .message(),
            "unsupported-target"
        );
    }
}

#[test]
fn listener_addresses_are_private_link_local_or_shared_vpn_space() {
    for address in [
        "10.0.0.1",
        "172.16.0.1",
        "192.168.31.101",
        "169.254.20.4",
        "100.64.0.1",
        "100.127.255.254",
    ] {
        assert!(
            connector_network_range(address.parse().unwrap()),
            "{address}"
        );
    }
    for address in [
        "0.0.0.0",
        "127.0.0.1",
        "8.8.8.8",
        "25.1.2.3",
        "100.63.255.255",
        "100.128.0.1",
        "224.0.0.1",
    ] {
        assert!(
            !connector_network_range(address.parse().unwrap()),
            "{address}"
        );
    }
    // Each non-RFC1918 range only on the interface kind it exists for.
    for (address, interface, eligible) in [
        ("192.168.31.101", "en0", true),
        ("10.1.2.3", "utun4", true),
        ("100.101.102.103", "utun4", true),
        ("100.101.102.103", "en0", false),
        ("100.101.102.103", "bridge0", false),
        ("169.254.20.4", "bridge0", true),
        ("169.254.20.4", "en0", false),
        ("169.254.20.4", "utun4", false),
        ("8.8.8.8", "utun4", false),
    ] {
        assert_eq!(
            connector_network_address(address.parse().unwrap(), interface),
            eligible,
            "{address} on {interface}"
        );
    }
    let local = ["192.168.31.101".parse().unwrap()];
    assert_eq!(
        validate_connector_listener("192.168.31.101", 9443, &local).unwrap(),
        local[0]
    );
    for address in ["", "0.0.0.0", "127.0.0.1", "8.8.8.8"] {
        assert!(
            validate_connector_listener(address, 9443, &local).is_err(),
            "{address:?} must never reach bind"
        );
    }
    assert!(validate_connector_listener("192.168.31.101", 80, &local).is_err());
    assert!(validate_connector_listener("10.0.0.2", 9443, &local).is_err());
}

#[test]
fn smoke_transport_accepts_only_authoritatively_absent_sessions() {
    assert!(require_smoke_session_stopped("target", Ok(String::new())).is_ok());
    assert!(require_smoke_session_stopped("target", Ok("other\n".into())).is_ok());
    assert_eq!(
        require_smoke_session_stopped("target", Ok("other\ntarget\n".into()))
            .unwrap_err()
            .message(),
        "smoke card must be stopped"
    );
    assert_eq!(
        require_smoke_session_stopped("target", Ok("malformed/name\n".into()))
            .unwrap_err()
            .message(),
        "smoke card state is unavailable"
    );
    for kind in [ErrorKind::NoSession, ErrorKind::Missing] {
        assert!(
            require_smoke_session_stopped("target", Err(DeckError::new(kind, "absent"))).is_ok()
        );
    }
    for kind in [ErrorKind::Tmux, ErrorKind::TmuxMissing, ErrorKind::Other] {
        assert_eq!(
            require_smoke_session_stopped("target", Err(DeckError::new(kind, "unknown")))
                .unwrap_err()
                .message(),
            "smoke card state is unavailable"
        );
    }
}

#[test]
fn journal_ids_are_device_scoped_immutable_and_cross_device_private() {
    let (r, _app) = test_runtime("journal");
    r.with_doc(|d| {
        d.devices.push(Device {
            id: "D1".into(),
            name: "one".into(),
            token_hash: "h".into(),
            paired_at: 1,
            revoked_at: None,
            history_pruned: false,
            retired_through: 0,
        });
        d.devices.push(Device {
            id: "D2".into(),
            name: "two".into(),
            token_hash: "h2".into(),
            paired_at: 1,
            revoked_at: None,
            history_pruned: false,
            retired_through: 0,
        });
        Ok(())
    })
    .unwrap();
    let one = r.accept(1, "D1", request("same", "a")).unwrap();
    let two = r.accept(1, "D2", request("same", "a")).unwrap();
    assert_eq!(one.state, "accepted");
    assert_eq!(two.state, "accepted");
    assert!(r.accept(1, "D1", request("same", "different")).is_err());
    assert!(r.command_result("D2", "missing").is_err());
    assert_eq!(r.read(|d| d.commands.len()).unwrap(), 2);

    let handle = sha(b"D1\0same");
    assert!(r.executing(&handle).is_err());
    r.with_doc(|d| {
        d.commands
            .iter_mut()
            .find(|c| c.handle == handle)
            .unwrap()
            .state = "executing".into();
        Ok(())
    })
    .unwrap();
    assert_eq!(r.executing(&handle).unwrap().request.id, "same");
    assert_eq!(r.command_result("D1", "same").unwrap().state, "accepted");
}

#[test]
fn executing_recovers_ambiguous_and_unresolved_capacity_is_bounded() {
    let (r, _app) = test_runtime("crash");
    r.with_doc(|d| {
        d.devices.push(Device {
            id: "D".into(),
            name: "device".into(),
            token_hash: "a".repeat(64),
            paired_at: 1,
            revoked_at: None,
            history_pruned: false,
            retired_through: 0,
        });
        let request = request("I", "a");
        d.commands.push(JournalEntry {
            handle: sha(b"D\0I"),
            device_id: "D".into(),
            request_hash: sha(&serde_json::to_vec(&request).unwrap()),
            id: request.id.clone(),
            kind: request.kind.clone(),
            seq: request.seq,
            request: Some(request),
            state: "executing".into(),
            code: None,
            result: None,
            accepted_at: 1,
            updated_at: 1,
        });
        Ok(())
    })
    .unwrap();
    let loaded = load(&r.path).unwrap();
    assert_eq!(loaded.commands[0].state, "ambiguous");
    let mut full = loaded;
    full.commands = (0..MAX_COMMANDS)
        .map(|i| JournalEntry {
            handle: format!("H{i}"),
            device_id: "D".into(),
            request_hash: "X".into(),
            id: request(&format!("I{i}"), "a").id.clone(),
            kind: request(&format!("I{i}"), "a").kind.clone(),
            seq: None,
            request: Some(request(&format!("I{i}"), "a")),
            state: "accepted".into(),
            code: None,
            result: None,
            accepted_at: 1,
            updated_at: 1,
        })
        .collect();
    *r.doc.lock_or_recover() = Ok(full);
    assert_eq!(
        r.accept(1, "D", request("new", "a")).unwrap_err().kind(),
        ErrorKind::DiskFull
    );
}

#[test]
fn terminal_history_does_not_consume_unresolved_command_capacity() {
    let (r, _app) = test_runtime("terminal-capacity");
    let history = (0..MAX_COMMANDS)
        .map(|i| request(&format!("I{i}"), "a"))
        .collect::<Vec<_>>();
    r.with_doc(|d| {
        d.devices.push(device("D"));
        d.commands = history
            .iter()
            .map(|request| tombstone("D", request, "rejected"))
            .collect();
        Ok(())
    })
    .unwrap();
    let accepted = r.accept(1, "D", request("after-history", "a")).unwrap();
    assert_eq!(accepted.state, "accepted");
    let replay = r.accept(1, "D", history[0].clone()).unwrap();
    assert_eq!(replay.state, "rejected");
    assert_eq!(
        r.accept(1, "D", request("I0", "different"))
            .unwrap_err()
            .kind(),
        ErrorKind::ContextChanged
    );
    assert_eq!(r.command_result("D", "I0").unwrap().state, "rejected");
}

#[test]
fn pairing_expires_consumes_once_and_revocation_blocks_auth() {
    let (r, _app) = test_runtime("pair");
    *r.pairing.lock_or_recover() = Some(Pairing {
        code: "secret".into(),
        expires_at: now() - 1,
    });
    assert!(r.pair(1, "secret", "phone").is_err());
    *r.pairing.lock_or_recover() = Some(Pairing {
        code: "secret".into(),
        expires_at: now() + 10,
    });
    let paired = r.pair(1, "secret", "phone").unwrap();
    let token = paired["token"].as_str().unwrap();
    let id = paired["deviceId"].as_str().unwrap();
    assert_eq!(r.active_device(token).as_deref(), Some(id));
    assert!(r.pair(1, "secret", "other").is_err());
    r.with_doc(|d| {
        d.devices
            .iter_mut()
            .find(|d| d.id == id)
            .unwrap()
            .revoked_at = Some(now());
        Ok(())
    })
    .unwrap();
    assert!(r.active_device(token).is_none());
}

#[test]
fn pairing_strips_invisible_device_name_characters() {
    let (runtime, _app) = test_runtime("pair-device-name");
    *runtime.pairing.lock_or_recover() = Some(Pairing {
        code: "secret".into(),
        expires_at: now() + 30,
    });
    assert_eq!(
        runtime
            .pair(1, "secret", "\u{202E}\u{200B}\u{E0001}")
            .unwrap_err()
            .message(),
        "invalid device name"
    );
    runtime
        .pair(1, "secret", "  My\u{202E} iPhone\u{200B}\u{E0001}  ")
        .unwrap();
    assert_eq!(
        runtime.read(|doc| doc.devices[0].name.clone()).unwrap(),
        "My iPhone"
    );
}

#[test]
fn pairing_is_consumed_once_under_concurrency() {
    let (runtime, _app) = test_runtime("pair-race");
    *runtime.pairing.lock_or_recover() = Some(Pairing {
        code: "secret".into(),
        expires_at: now() + 30,
    });
    let gate = Arc::new(std::sync::Barrier::new(3));
    let mut workers = Vec::new();
    for name in ["one", "two"] {
        let runtime = runtime.clone();
        let gate = gate.clone();
        workers.push(std::thread::spawn(move || {
            gate.wait();
            runtime.pair(1, "secret", name).is_ok()
        }));
    }
    gate.wait();
    let successes = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .filter(|success| *success)
        .count();
    assert_eq!(successes, 1);
    assert_eq!(runtime.read(|doc| doc.devices.len()).unwrap(), 1);
    assert!(runtime.pairing.lock_or_recover().is_none());
}

#[test]
fn disable_and_revocation_close_existing_epochs_and_pending_work() {
    let (runtime, _app) = test_runtime("lifecycle");
    let token = "token";
    runtime
        .with_doc(|doc| {
            doc.devices.push(Device {
                id: "D".into(),
                name: "device".into(),
                token_hash: sha(format!("deck-device-v1\0{token}").as_bytes()),
                paired_at: 1,
                revoked_at: None,
                history_pruned: false,
                retired_through: 0,
            });
            Ok(())
        })
        .unwrap();
    runtime.accept(1, "D", request("accepted", "a")).unwrap();
    runtime.accept(1, "D", request("executing", "b")).unwrap();
    runtime
        .with_doc(|doc| {
            doc.commands[1].state = "executing".into();
            Ok(())
        })
        .unwrap();
    assert_eq!(runtime.authorize(1, token).unwrap(), "D");
    runtime.server_epoch.store(2, Ordering::SeqCst);
    assert_eq!(
        runtime.authorize(1, token).unwrap_err().kind(),
        ErrorKind::Perm
    );
    runtime
        .with_doc(|doc| {
            doc.config.enabled = false;
            invalidate_commands(doc, None, "connector-disabled");
            Ok(())
        })
        .unwrap();
    assert_eq!(
        runtime.read(|doc| doc.commands[0].state.clone()).unwrap(),
        "rejected"
    );
    assert_eq!(
        runtime.read(|doc| doc.commands[1].state.clone()).unwrap(),
        "ambiguous"
    );
}

#[test]
fn generated_certificate_has_ip_san_and_real_rustls_verification() {
    let identity = Identity::generate("127.0.0.1").unwrap();
    let encoded = identity.encode().unwrap();
    let decoded = Identity::decode(&encoded).unwrap();
    assert_eq!(decoded.address, "127.0.0.1");
    assert_eq!(decoded.fingerprint, identity.fingerprint);
    assert!(Identity::decode("not-base64").is_err());
    let cert = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&decoded.cert_der)
        .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(cert)).unwrap();
    let client = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let server = Arc::new(decoded.tls().unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let worker = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        let conn = rustls::ServerConnection::new(server).unwrap();
        let mut stream = rustls::StreamOwned::new(conn, tcp);
        let mut b = [0; 1];
        stream.read_exact(&mut b).unwrap();
        stream.write_all(b"y").unwrap();
    });
    let tcp = TcpStream::connect(addr).unwrap();
    let name = ServerName::IpAddress("127.0.0.1".parse::<IpAddr>().unwrap().into());
    let conn = rustls::ClientConnection::new(client, name).unwrap();
    let mut stream = rustls::StreamOwned::new(conn, tcp);
    stream.write_all(b"x").unwrap();
    let mut b = [0; 1];
    stream.read_exact(&mut b).unwrap();
    assert_eq!(&b, b"y");
    worker.join().unwrap();
}

// ---- F4: admission-side replay protection ----

fn reload(r: &Runtime) -> Arc<Runtime> {
    Arc::new(Runtime {
        app: None,
        path: r.path.clone(),
        doc: Mutex::new(load(&r.path)),
        pairing: Mutex::new(None),
        lifecycle: Mutex::new(()),
        server_epoch: AtomicU64::new(1),
        running_epoch: AtomicU64::new(1),
    })
}

fn resolve(r: &Runtime, device_id: &str, id: &str, state: &str) {
    let handle = sha(format!("{device_id}\0{id}").as_bytes());
    r.with_doc(|d| {
        let c = d.commands.iter_mut().find(|c| c.handle == handle).unwrap();
        c.state = state.into();
        if state == "applied" {
            c.result = Some(json!({"cardId":"C1","entryId":"E1","revision":"2"}));
        }
        Ok(())
    })
    .unwrap();
}

/// Accept and finish `first`, then push it out of the tombstone history
/// with later finished commands of the same device.
fn retire(r: &Runtime, first: &CommandRequest) {
    r.accept(1, "D", first.clone()).unwrap();
    resolve(r, "D", &first.id, "applied");
    let later = (0..MAX_TOMBSTONES)
        .map(|i| tombstone("D", &request(&format!("L{i}"), "later"), "rejected"))
        .collect::<Vec<_>>();
    r.with_doc(|d| {
        d.commands.extend(later);
        Ok(())
    })
    .unwrap();
    let doc = r.read(Clone::clone).unwrap();
    assert!(doc.commands.iter().all(|c| c.id != first.id), "not retired");
    let device = doc.devices.iter().find(|d| d.id == "D").unwrap();
    assert_eq!(device.retired_through, first.seq.unwrap());
}

fn accepted_count(r: &Runtime) -> usize {
    r.read(|d| d.commands.iter().filter(|c| unresolved(&c.state)).count())
        .unwrap()
}

#[test]
fn f4_a_retired_command_is_never_admitted_again() {
    let (r, _app) = test_runtime("f4-retired");
    r.with_doc(|d| {
        d.devices.push(device("D"));
        Ok(())
    })
    .unwrap();
    let first = request("first", "send once");
    retire(&r, &first);
    let before = r.read(|d| d.commands.len()).unwrap();
    // Exact replay, without asking GET first.
    let replay = r.accept(1, "D", first.clone()).unwrap_err();
    assert_eq!(replay.message(), COMMAND_EXPIRED);
    assert_eq!(r.read(|d| d.commands.len()).unwrap(), before);
    assert_eq!(
        accepted_count(&r),
        0,
        "a retired command was admitted again"
    );
    // The same identity with another body: refused, never compared.
    let mut changed = first.clone();
    changed.payload = json!({"text":"changed"});
    assert_eq!(
        r.accept(1, "D", changed).unwrap_err().message(),
        COMMAND_EXPIRED
    );
    // A new command in the same pruned state still works.
    let fresh = r.accept(1, "D", request("fresh", "new work")).unwrap();
    assert_eq!(fresh.state, "accepted");
    // Inside the window a reused id conflicts; a reused seq conflicts.
    let mut conflict = request("fresh", "other body");
    conflict.seq = Some(next_seq());
    assert_eq!(
        r.accept(1, "D", conflict).unwrap_err().kind(),
        ErrorKind::ContextChanged
    );
    let fresh_seq = r
        .read(|d| d.commands.iter().find(|c| c.id == "fresh").unwrap().seq)
        .unwrap();
    let mut reused = request("reused-seq", "x");
    reused.seq = fresh_seq;
    assert_eq!(
        r.accept(1, "D", reused).unwrap_err().kind(),
        ErrorKind::ContextChanged
    );
    // A phone build without sequences cannot be admitted at all.
    let mut legacy = request("legacy", "x");
    legacy.seq = None;
    assert_eq!(
        r.accept(1, "D", legacy).unwrap_err().message(),
        CLIENT_UPGRADE_REQUIRED
    );
    // Restart keeps the floor.
    let reloaded = reload(&r);
    assert_eq!(
        reloaded.accept(1, "D", first).unwrap_err().message(),
        COMMAND_EXPIRED
    );
    assert_eq!(accepted_count(&reloaded), 1, "only `fresh` is pending");
}

#[test]
fn f4_concurrent_identical_posts_admit_once() {
    let (r, _app) = test_runtime("f4-concurrent");
    r.with_doc(|d| {
        d.devices.push(device("D"));
        Ok(())
    })
    .unwrap();
    let command = request("same", "once");
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let threads = (0..4)
        .map(|_| {
            let (r, command, barrier) = (r.clone(), command.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                r.accept(1, "D", command).unwrap().state
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        assert_eq!(thread.join().unwrap(), "accepted");
    }
    assert_eq!(r.read(|d| d.commands.len()).unwrap(), 1);
}

#[test]
fn f4_crash_boundaries_never_admit_twice_or_guess_success() {
    let (r, _app) = test_runtime("f4-crash");
    r.with_doc(|d| {
        d.devices.push(device("D"));
        Ok(())
    })
    .unwrap();
    // 1. Admission persisted, not yet dispatched, then a restart.
    let waiting = request("waiting", "a");
    r.accept(1, "D", waiting.clone()).unwrap();
    let r = reload(&r);
    assert_eq!(r.accept(1, "D", waiting).unwrap().state, "accepted");
    assert_eq!(accepted_count(&r), 1);
    // 2. Dispatched (executing), result never persisted, then a restart.
    let dispatched = request("dispatched", "b");
    r.accept(1, "D", dispatched.clone()).unwrap();
    resolve(&r, "D", "dispatched", "executing");
    let r = reload(&r);
    let answer = r.accept(1, "D", dispatched).unwrap();
    assert_eq!(
        answer.state, "ambiguous",
        "an unknown outcome is not guessed"
    );
    // 3. Result saved, HTTP receipt lost.
    let finished = request("finished", "c");
    r.accept(1, "D", finished.clone()).unwrap();
    resolve(&r, "D", "finished", "applied");
    assert_eq!(r.accept(1, "D", finished).unwrap().state, "applied");
    assert_eq!(r.read(|d| d.commands.len()).unwrap(), 3);
    assert_eq!(accepted_count(&r), 1, "only the first is still pending");
}
