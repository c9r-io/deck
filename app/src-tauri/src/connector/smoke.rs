//! Debug-only smoke hooks: seeded state, transport path and window visibility.
//!
//! Split out of the one-file `connector/mod.rs` on 2026-09-23; the contract
//! stays in `connector/mod.rs`.

use super::*;

#[tauri::command]
pub(crate) fn connector_smoke_seed(
    card_id: String,
    expected_revision: String,
) -> Result<PendingView, DeckError> {
    if !crate::smoke_faults::enabled() || !command_id(&card_id) {
        return Err(DeckError::new(
            ErrorKind::Other,
            "smoke hooks are unavailable",
        ));
    }
    let runtime = rt()?;
    runtime.with_doc(|doc| {
        doc.config = Config {
            enabled: true,
            address: "127.0.0.1".into(),
            port: 18443,
            interface: None,
        };
        if !doc.devices.iter().any(|device| device.id == "device_smoke") {
            doc.devices.push(Device {
                id: "device_smoke".into(),
                name: "Smoke device".into(),
                token_hash: "a".repeat(64),
                paired_at: now(),
                revoked_at: None,
                history_pruned: false,
                retired_through: 0,
            });
        }
        Ok(())
    })?;
    let epoch = runtime.server_epoch.fetch_add(1, Ordering::SeqCst) + 1;
    runtime.running_epoch.store(epoch, Ordering::SeqCst);
    let request = CommandRequest {
        id: random_id("smoke_", 8)?,
        kind: "buffer-add".into(),
        card_id: Some(card_id),
        expected_generation: ExpectedGeneration::Missing,
        expected_revision: Some(expected_revision),
        payload: json!({"text":"smoke connector note"}),
        // Debug-only seed: any fresh positive sequence is valid.
        seq: Some(u64::from_be_bytes(random(8)?.try_into().unwrap_or([0; 8])) >> 1 | 1),
    };
    let handle = sha(format!("device_smoke\0{}", request.id).as_bytes());
    runtime.accept(epoch, "device_smoke", request.clone())?;
    Ok(PendingView { handle, request })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SmokeTransportView {
    pub(super) path: String,
}

pub(super) fn require_smoke_session_stopped(
    session: &str,
    result: Result<String, DeckError>,
) -> Result<(), DeckError> {
    match result {
        Ok(sessions) => {
            let mut live = false;
            for listed in sessions.lines() {
                if listed.is_empty() || crate::tmux::validate_session_name(listed).is_err() {
                    return Err(DeckError::new(
                        ErrorKind::Other,
                        "smoke card state is unavailable",
                    ));
                }
                live |= listed == session;
            }
            if live {
                Err(DeckError::new(
                    ErrorKind::Invalid,
                    "smoke card must be stopped",
                ))
            } else {
                Ok(())
            }
        }
        Err(error) if matches!(error.kind(), ErrorKind::NoSession | ErrorKind::Missing) => Ok(()),
        Err(_) => Err(DeckError::new(
            ErrorKind::Other,
            "smoke card state is unavailable",
        )),
    }
}

#[tauri::command]
pub(crate) fn connector_smoke_transport(card_id: String) -> Result<SmokeTransportView, DeckError> {
    if !crate::smoke_faults::enabled() || !command_id(&card_id) {
        return Err(DeckError::new(
            ErrorKind::Other,
            "smoke hooks are unavailable",
        ));
    }
    let card = committed_card(&card_id)?;
    require_smoke_session_stopped(
        &card.session,
        crate::tmux::tmux(&["list-sessions", "-F", "#{session_name}"]),
    )?;

    let runtime = rt()?.clone();
    let identity = Identity::generate("127.0.0.1")?;
    let port;
    {
        let _lifecycle = runtime.lifecycle.lock_or_recover();
        runtime.server_epoch.fetch_add(1, Ordering::SeqCst);
        *runtime.pairing.lock_or_recover() = None;
        runtime.with_doc(|doc| {
            doc.config = Config {
                enabled: false,
                address: "127.0.0.1".into(),
                port: 0,
                interface: None,
            };
            doc.identity_address = Some(identity.address.clone());
            doc.identity_fingerprint = Some(identity.fingerprint.clone());
            invalidate_commands(doc, None, "connector-disabled");
            Ok(())
        })?;
        keychain::set(Slot::ConnectorIdentity, &identity.encode()?)?;
        let epoch = runtime.server_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        port = server::spawn(
            runtime.clone(),
            Config {
                enabled: true,
                address: "127.0.0.1".into(),
                port: 0,
                interface: None,
            },
            identity,
            epoch,
            // Loopback smoke listener: no network to change.
            || true,
        )?;
        if let Err(error) = runtime.with_doc(|doc| {
            doc.config = Config {
                enabled: true,
                address: "127.0.0.1".into(),
                port,
                interface: None,
            };
            Ok(())
        }) {
            runtime.server_epoch.fetch_add(1, Ordering::SeqCst);
            return Err(error);
        }
    }

    let pairing = connector_pairing()?;
    let path = crate::datadir::deck_dir().join("connector-smoke-transport.json");
    let bytes = serde_json::to_vec(&json!({
        "pairingURI": pairing.uri,
        "cardId": card_id,
    }))
    .map_err(|_| DeckError::new(ErrorKind::Other, "smoke fixture encoding failed"))?;
    if let Err(error) = crate::datadir::atomic_write(&path, &bytes) {
        let _ = connector_disable();
        return Err(error);
    }
    let path = path
        .to_str()
        .ok_or_else(|| DeckError::new(ErrorKind::Other, "smoke fixture path is invalid"))?
        .to_owned();
    Ok(SmokeTransportView { path })
}

#[tauri::command]
pub(crate) fn connector_smoke_window(visible: bool, app: AppHandle) -> Result<(), DeckError> {
    if !crate::smoke_faults::enabled() {
        return Err(DeckError::new(
            ErrorKind::Other,
            "smoke hooks are unavailable",
        ));
    }
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "main window unavailable"))?;
    if visible {
        window.show()
    } else {
        window.hide()
    }
    .map_err(|_| DeckError::new(ErrorKind::Other, "window state unavailable"))
}
