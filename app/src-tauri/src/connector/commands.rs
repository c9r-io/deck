//! Local Tauri commands: status, enable/disable, pairing, revocation, pending intents, claim/complete/validate, native execution.
//!
//! Split out of the one-file `connector/mod.rs` on 2026-09-23; the contract
//! stays in `connector/mod.rs`. `connector_validate` and
//! `connector_validate_admission` are thin over `validate_claimed` and
//! `validate_claimed_admission`, which take the board-side check (or the
//! board loader) as an argument so tests never read the committed board.

use super::*;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    pub(super) enabled: bool,
    pub(super) running: bool,
    pub(super) address: String,
    pub(super) port: u16,
    pub(super) origin: Option<String>,
    pub(super) fingerprint: Option<String>,
    pub(super) reset_required: bool,
    pub(super) devices: Vec<DeviceView>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeviceView {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) paired_at: u64,
    pub(super) revoked: bool,
}

#[tauri::command]
pub(crate) fn connector_addresses() -> Vec<String> {
    local_ipv4_addresses()
        .into_iter()
        .map(|x| x.to_string())
        .collect()
}
#[tauri::command]
pub(crate) fn connector_status() -> Result<Status, DeckError> {
    let r = rt()?;
    r.read(|d| Status {
        enabled: d.config.enabled,
        running: d.config.enabled
            && r.running_epoch.load(Ordering::SeqCst) != 0
            && r.running_epoch.load(Ordering::SeqCst) == r.server_epoch.load(Ordering::SeqCst),
        address: d.config.address.clone(),
        port: d.config.port,
        origin: d
            .config
            .enabled
            .then(|| format!("https://{}:{}", d.config.address, d.config.port)),
        fingerprint: d.identity_fingerprint.clone(),
        reset_required: d
            .identity_address
            .as_ref()
            .is_some_and(|address| !d.config.address.is_empty() && address != &d.config.address),
        devices: d
            .devices
            .iter()
            .map(|x| DeviceView {
                id: x.id.clone(),
                name: x.name.clone(),
                paired_at: x.paired_at,
                revoked: x.revoked_at.is_some(),
            })
            .collect(),
    })
}

#[tauri::command]
pub(crate) async fn connector_enable(address: String, port: u16) -> Result<Status, DeckError> {
    tauri::async_runtime::spawn_blocking(move || {
        let ip = validate_connector_listener(&address, port, &local_ipv4_addresses())?;
        let r = rt()?.clone();
        let _lifecycle = r.lifecycle.lock_or_recover();
        let already_running = r
            .read(|d| d.config.enabled && d.config.address == address && d.config.port == port)?
            && r.feature_active();
        if already_running {
            return connector_status();
        }
        let identity = match identity_get()? {
            Some(i) if i.address == address => i,
            Some(_) => {
                return Err(DeckError::new(
                    ErrorKind::ContextChanged,
                    "connector identity reset required",
                ))
            }
            None => {
                let i = Identity::generate(&address)?;
                keychain::set(Slot::ConnectorIdentity, &i.encode()?)?;
                i
            }
        };
        let interface = interface_of(ip, &local_ipv4_interfaces());
        r.with_doc(|d| {
            d.config = Config {
                enabled: true,
                address: address.clone(),
                port,
                interface,
            };
            d.identity_address = Some(identity.address.clone());
            d.identity_fingerprint = Some(identity.fingerprint.clone());
            Ok(())
        })?;
        if let Err(error) = start_server(r.clone()) {
            let _ = r.with_doc(|d| {
                d.config.enabled = false;
                Ok(())
            });
            return Err(error);
        }
        connector_status()
    })
    .await
    .map_err(|_| DeckError::new(ErrorKind::Other, "connector worker failed"))?
}

#[tauri::command]
pub(crate) fn connector_disable() -> Result<(), DeckError> {
    let r = rt()?;
    let _lifecycle = r.lifecycle.lock_or_recover();
    r.server_epoch.fetch_add(1, Ordering::SeqCst);
    *r.pairing.lock_or_recover() = None;
    r.with_doc(|d| {
        d.config.enabled = false;
        invalidate_commands(d, None, "connector-disabled");
        Ok(())
    })
}
#[tauri::command]
pub(crate) fn connector_reset_identity() -> Result<(), DeckError> {
    let r = rt()?;
    let _lifecycle = r.lifecycle.lock_or_recover();
    if r.read(|d| d.config.enabled)? {
        return Err(DeckError::new(
            ErrorKind::Other,
            "disable connector before reset",
        ));
    }
    r.with_doc(|d| {
        d.devices.clear();
        d.commands.clear();
        d.identity_address = None;
        d.identity_fingerprint = None;
        Ok(())
    })?;
    keychain::clear(Slot::ConnectorIdentity)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairingView {
    pub(super) uri: String,
    pub(super) svg: String,
    pub(super) expires_at: u64,
    pub(super) origin: String,
    pub(super) fingerprint: String,
}
#[tauri::command]
pub(crate) fn connector_pairing() -> Result<PairingView, DeckError> {
    let r = rt()?;
    let _lifecycle = r.lifecycle.lock_or_recover();
    let (host_id, cfg) = r.read(|d| (d.host_id.clone(), d.config.clone()))?;
    if !cfg.enabled {
        return Err(DeckError::new(ErrorKind::Other, "connector is disabled"));
    }
    if r.running_epoch.load(Ordering::SeqCst) == 0
        || r.running_epoch.load(Ordering::SeqCst) != r.server_epoch.load(Ordering::SeqCst)
    {
        return Err(DeckError::new(
            ErrorKind::Other,
            "connector is not listening",
        ));
    }
    let identity = identity_get()?
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "connector identity is missing"))?;
    let code = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random(18)?);
    let expires_at = now() + PAIR_TTL;
    *r.pairing.lock_or_recover() = Some(Pairing {
        code: code.clone(),
        expires_at,
    });
    let origin = format!("https://{}:{}", cfg.address, cfg.port);
    let Some(uri) = pairing_descriptor(
        &host_id,
        &host_name(),
        &origin,
        &identity.fingerprint,
        &code,
        expires_at,
    ) else {
        *r.pairing.lock_or_recover() = None;
        return Err(DeckError::new(
            ErrorKind::Other,
            "pairing descriptor is too large",
        ));
    };
    let svg = qrcode::QrCode::new(uri.as_bytes())
        .map_err(|_| DeckError::new(ErrorKind::Other, "pairing QR failed"))?
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(256, 256)
        .build();
    Ok(PairingView {
        uri,
        svg,
        expires_at,
        origin,
        fingerprint: identity.fingerprint,
    })
}
#[tauri::command]
pub(crate) fn connector_revoke(device_id: String) -> Result<(), DeckError> {
    rt()?.with_doc(|d| revoke_device(d, &device_id))
}

pub(super) fn revoke_device(d: &mut DiskDoc, device_id: &str) -> Result<(), DeckError> {
    let x = d
        .devices
        .iter_mut()
        .find(|x| x.id == device_id)
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "device not found"))?;
    x.revoked_at = Some(now());
    // A revoked token can never query or replay its history, so its
    // tombstones are dropped. An entry that was executing stays (as
    // ambiguous) until the device record itself is pruned, so an in-flight
    // native step still finds its journal entry.
    let in_flight = d
        .commands
        .iter()
        .filter(|c| c.device_id == device_id && c.state == "executing")
        .map(|c| c.handle.clone())
        .collect::<HashSet<_>>();
    invalidate_commands(d, Some(device_id), "device-revoked");
    d.commands
        .retain(|c| c.device_id != device_id || in_flight.contains(&c.handle));
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingView {
    pub(super) handle: String,
    pub(super) request: CommandRequest,
}
pub(super) struct ExecutingCommand {
    pub(super) device_id: String,
    pub(super) request: CommandRequest,
}
#[tauri::command]
pub(crate) fn connector_pending() -> Result<Vec<PendingView>, DeckError> {
    rt()?.read(|d| {
        d.commands
            .iter()
            .filter(|c| c.state == "accepted")
            .filter_map(|c| {
                Some(PendingView {
                    handle: c.handle.clone(),
                    request: c.request.clone()?,
                })
            })
            .collect()
    })
}

#[tauri::command]
pub(crate) fn connector_claim(handle: String) -> Result<PendingView, DeckError> {
    let r = rt()?;
    let _lifecycle = r.lifecycle.lock_or_recover();
    if !r.feature_active() {
        return Err(DeckError::new(ErrorKind::Perm, "connector unavailable"));
    }
    r.with_doc(|d| {
        let c = d
            .commands
            .iter_mut()
            .find(|c| c.handle == handle)
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "command not found"))?;
        if !d
            .devices
            .iter()
            .any(|x| x.id == c.device_id && x.revoked_at.is_none())
        {
            return Err(DeckError::new(ErrorKind::Perm, "device revoked"));
        }
        if c.state != "accepted" {
            return Err(DeckError::new(ErrorKind::Other, "command is not pending"));
        }
        let request = c.body()?.clone();
        c.state = "executing".into();
        c.updated_at = now();
        Ok(PendingView {
            handle: c.handle.clone(),
            request,
        })
    })
}
#[tauri::command]
pub(crate) fn connector_complete(
    handle: String,
    state: String,
    code: Option<String>,
    result: Option<Value>,
) -> Result<(), DeckError> {
    if !matches!(
        state.as_str(),
        "applied" | "delivered" | "rejected" | "ambiguous"
    ) {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "invalid command result state",
        ));
    }
    rt()?.with_doc(|d| {
        let c = d
            .commands
            .iter_mut()
            .find(|c| c.handle == handle)
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "command not found"))?;
        if c.state != "executing" {
            return Err(DeckError::new(ErrorKind::Other, "command is not executing"));
        }
        if !validate_terminal(&c.kind, &state, code.as_deref(), result.as_ref()) {
            return Err(DeckError::new(
                ErrorKind::Invalid,
                "command result is invalid",
            ));
        }
        c.state = state;
        c.code = code;
        c.result = result;
        c.updated_at = now();
        Ok(())
    })
}

#[tauri::command]
pub(crate) fn connector_validate(handle: String) -> Result<bool, DeckError> {
    validate_claimed(&handle, validate_applicable)
}

/// The claimed (executing, still authorized) request behind `handle`, run
/// through `applicable` (the committed-board check in production).
pub(super) fn validate_claimed(
    handle: &str,
    applicable: impl FnOnce(&CommandRequest) -> Result<(), DeckError>,
) -> Result<bool, DeckError> {
    let r = rt()?;
    let _lifecycle = r.lifecycle.lock_or_recover();
    if !r.feature_active() {
        return Err(DeckError::new(ErrorKind::Perm, "connector unavailable"));
    }
    let request = r.read(|d| {
        let c = d
            .commands
            .iter()
            .find(|c| c.handle == handle)
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "command not found"))?;
        if c.state != "executing"
            || !d
                .devices
                .iter()
                .any(|x| x.id == c.device_id && x.revoked_at.is_none())
        {
            return Err(DeckError::new(
                ErrorKind::ContextChanged,
                "command authorization changed",
            ));
        }
        c.body().cloned()
    })??;
    applicable(&request)?;
    Ok(true)
}

#[tauri::command]
pub(crate) fn connector_validate_admission(handle: String) -> Result<bool, DeckError> {
    validate_claimed_admission(&handle, board_value)
}

/// Admission proof for the claimed buffer-queue command behind `handle`
/// against the board `board` loads (the committed board in production).
pub(super) fn validate_claimed_admission(
    handle: &str,
    board: impl FnOnce() -> Result<(String, Value), DeckError>,
) -> Result<bool, DeckError> {
    if handle.len() != 64 || !handle.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DeckError::new(ErrorKind::Invalid, "invalid command handle"));
    }
    let runtime = rt()?;
    let _lifecycle = runtime.lifecycle.lock_or_recover();
    if !runtime.feature_active() {
        return Err(DeckError::new(ErrorKind::Perm, "connector unavailable"));
    }
    let request = runtime.read(|doc| {
        let command = doc
            .commands
            .iter()
            .find(|command| command.handle == handle)
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "command not found"))?;
        if command.state != "executing"
            || !doc
                .devices
                .iter()
                .any(|device| device.id == command.device_id && device.revoked_at.is_none())
        {
            return Err(DeckError::new(
                ErrorKind::ContextChanged,
                "command authorization changed",
            ));
        }
        command.body().cloned()
    })??;
    let (_, board) = board()?;
    validate_admission_board(handle, &request, &board)?;
    Ok(true)
}

#[tauri::command]
pub(crate) async fn connector_execute_native(
    handle: String,
    app: AppHandle,
) -> Result<(), DeckError> {
    tauri::async_runtime::spawn_blocking(move || {
        let _deadline = crate::session_runtime::Deadline::until(
            std::time::Instant::now() + std::time::Duration::from_secs(15),
        );
        let runtime = rt()?;
        if !runtime.feature_active() {
            return connector_complete(
                handle,
                "ambiguous".into(),
                Some("connector-disabled".into()),
                None,
            );
        }
        let executing = runtime.executing(&handle)?;
        let queues = app.state::<Queues>();
        let result = execute_native(&executing.request, &handle, &executing.device_id, &queues);
        match result {
            Ok(state) => connector_complete(handle, state.into(), None, None),
            Err((state, code)) => connector_complete(handle, state.into(), Some(code.into()), None),
        }
    })
    .await
    .map_err(|_| DeckError::new(ErrorKind::Other, "connector worker failed"))?
}

/// The QR payload the phone parses (`PairingDescriptor.swift`); `None` when
/// it would exceed the phone's 8 KiB descriptor bound. Golden:
/// `connector/ios/Tests/DeckConnectorCoreTests/Fixtures/pairing-descriptor.txt`.
pub(super) fn pairing_descriptor(
    host_id: &str,
    host_name: &str,
    origin: &str,
    fingerprint: &str,
    code: &str,
    expires_at: u64,
) -> Option<String> {
    let data = json!({"version":1,"hostId":host_id,"hostName":host_name,"origin":origin,"fingerprint":fingerprint,"code":code,"expiresAt":expires_at});
    let encoded =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&data).ok()?);
    (encoded.len() <= MAX_PAIRING_DESCRIPTOR_BYTES)
        .then(|| format!("deck-connector://pair?data={encoded}"))
}
