//! Journal engine: load/upgrade, compaction, encode/persist, admission budget, accept/resolve and device pruning.
//!
//! Split out of the one-file `connector/mod.rs` on 2026-09-23; the contract
//! stays in `connector/mod.rs`.

use super::*;

pub(super) fn external_state(state: &str) -> String {
    if state == "executing" {
        "accepted".into()
    } else {
        state.into()
    }
}

pub(super) fn invalidate_commands(doc: &mut DiskDoc, device_id: Option<&str>, code: &str) {
    let at = now();
    for command in &mut doc.commands {
        if device_id.is_some_and(|id| command.device_id != id) {
            continue;
        }
        match command.state.as_str() {
            "accepted" => {
                command.state = "rejected".into();
                command.code = Some(code.into());
                command.updated_at = at;
            }
            "executing" => {
                command.state = "ambiguous".into();
                command.code = Some(code.into());
                command.updated_at = at;
            }
            _ => {}
        }
    }
}

pub(super) fn load(path: &Path) -> Result<DiskDoc, DeckError> {
    let Some(bytes) = crate::ledger::read_bounded(path, MAX_STATE_BYTES, "connector state")? else {
        return DiskDoc::fresh();
    };
    let mut doc: DiskDoc = crate::ledger::decode(&bytes, "connector state")?;
    if !(1..=VERSION).contains(&doc.version)
        || doc.devices.len() > MAX_DEVICES
        || doc.host_id.is_empty()
        || doc
            .identity_address
            .as_ref()
            .is_some_and(|v| v.parse::<Ipv4Addr>().is_err())
        || doc
            .identity_fingerprint
            .as_ref()
            .is_some_and(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
        || doc.identity_address.is_some() != doc.identity_fingerprint.is_some()
        || (doc.config.enabled
            && (doc.config.port < 1024 || doc.config.address.parse::<Ipv4Addr>().is_err()))
        || doc
            .config
            .interface
            .as_ref()
            .is_some_and(|name| !interface_name(name))
    {
        return Err(DeckError::new(
            ErrorKind::Recovery,
            "connector state is invalid",
        ));
    }
    let migrating = doc.version == 1;
    if migrating {
        // A v1 entry always carries its request and has no id/kind copy.
        if doc
            .commands
            .iter()
            .any(|c| !c.id.is_empty() || !c.kind.is_empty() || c.request.is_none())
            || doc.config.interface.is_some()
            || doc.devices.iter().any(|d| d.history_pruned)
        {
            return Err(DeckError::new(
                ErrorKind::Recovery,
                "connector journal records are invalid",
            ));
        }
        for c in &mut doc.commands {
            if let Some(request) = &c.request {
                c.id = request.id.clone();
                c.kind = request.kind.clone();
            }
        }
    }
    let mut device_ids = HashSet::new();
    if doc.devices.iter().any(|d| {
        !device_ids.insert(&d.id)
            || d.id.is_empty()
            || d.name.is_empty()
            || d.name.chars().any(char::is_control)
            || d.token_hash.len() != 64
            || !d.token_hash.bytes().all(|b| b.is_ascii_hexdigit())
    }) {
        return Err(DeckError::new(
            ErrorKind::Recovery,
            "connector device records are invalid",
        ));
    }
    let mut handles = HashSet::new();
    if doc.commands.iter().any(|c| {
        !handles.insert(&c.handle)
            || !device_ids.contains(&c.device_id)
            || !command_id(&c.id)
            || !command_kind(&c.kind)
            || c.handle != sha(format!("{}\0{}", c.device_id, c.id).as_bytes())
            || !validate_terminal(&c.kind, &c.state, c.code.as_deref(), c.result.as_ref())
            || match &c.request {
                Some(request) => {
                    request.id != c.id
                        || request.kind != c.kind
                        || serde_json::to_vec(request)
                            .map(|bytes| c.request_hash != sha(&bytes))
                            .unwrap_or(true)
                        || validate_command(request).is_err()
                        || !(migrating || unresolved(&c.state))
                }
                None => {
                    unresolved(&c.state)
                        || c.request_hash.len() != 64
                        || !c.request_hash.bytes().all(|b| b.is_ascii_hexdigit())
                }
            }
    }) {
        return Err(DeckError::new(
            ErrorKind::Recovery,
            "connector journal records are invalid",
        ));
    }
    let pending = doc.commands.iter().filter(|c| unresolved(&c.state)).count();
    let tombstones = doc.commands.len() - pending;
    if pending > MAX_COMMANDS
        || (!migrating && tombstones > MAX_TOMBSTONES)
        || over_admission_budget(bytes.len(), &doc)
    {
        return Err(DeckError::new(
            ErrorKind::Recovery,
            "connector state exceeds its reserved capacity",
        ));
    }
    doc.version = VERSION;
    let mut changed = false;
    for c in &mut doc.commands {
        if c.state == "executing" {
            c.state = "ambiguous".into();
            c.code = Some("interrupted".into());
            c.updated_at = now();
            changed = true;
        }
    }
    compact(&mut doc);
    if changed {
        save(path, &doc)?;
    }
    Ok(doc)
}

pub(super) fn unresolved(state: &str) -> bool {
    matches!(state, "accepted" | "executing")
}

pub(super) fn command_kind(kind: &str) -> bool {
    matches!(
        kind,
        "send-message"
            | "buffer-add"
            | "buffer-edit"
            | "buffer-delete"
            | "buffer-queue"
            | "task-create"
            | "queue-pause"
            | "queue-cancel"
    )
}

/// Reduce every resolved entry to its tombstone and bound the tombstone
/// history. Runs on every committed write, so a terminal entry is never
/// persisted with its request body. Dropping the oldest tombstone marks its
/// device: that device's unknown ids are then answered `expired`.
pub(super) fn compact(doc: &mut DiskDoc) {
    for c in &mut doc.commands {
        if !unresolved(&c.state) {
            c.request = None;
        }
    }
    let mut excess = doc
        .commands
        .iter()
        .filter(|c| c.request.is_none())
        .count()
        .saturating_sub(MAX_TOMBSTONES);
    if excess == 0 {
        return;
    }
    let mut pruned = std::collections::HashMap::<String, u64>::new();
    doc.commands.retain(|c| {
        if excess > 0 && c.request.is_none() {
            excess -= 1;
            let floor = pruned.entry(c.device_id.clone()).or_default();
            *floor = (*floor).max(c.seq.unwrap_or(0));
            false
        } else {
            true
        }
    });
    for device in &mut doc.devices {
        if let Some(floor) = pruned.get(&device.id) {
            device.history_pruned = true;
            device.retired_through = device.retired_through.max(*floor);
        }
    }
}

#[cfg(test)]
thread_local! {
    pub(super) static ENCODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) fn encode(doc: &DiskDoc) -> Result<Vec<u8>, DeckError> {
    #[cfg(test)]
    ENCODES.with(|count| count.set(count.get() + 1));
    crate::ledger::encode(doc, "connector state")
}

pub(super) fn save(path: &Path, doc: &DiskDoc) -> Result<(), DeckError> {
    persist(path, doc, false)
}

/// Encode once, check the byte cap (plus, for an admission, the terminal
/// reserve of every unresolved command) and atomically replace the file:
/// unique temp file, file fsync, rename, parent-directory fsync.
pub(super) fn persist(path: &Path, doc: &DiskDoc, admission: bool) -> Result<(), DeckError> {
    let bytes = encode(doc)?;
    if admission && over_admission_budget(bytes.len(), doc) {
        return Err(DeckError::new(
            ErrorKind::DiskFull,
            "connector state capacity reached",
        ));
    }
    crate::ledger::write_bounded(path, &bytes, MAX_STATE_BYTES, "connector state")
}

pub(super) fn over_admission_budget(encoded_len: usize, doc: &DiskDoc) -> bool {
    let pending = doc.commands.iter().filter(|c| unresolved(&c.state)).count();
    encoded_len.saturating_add(pending.saturating_mul(TERMINAL_RESERVE_BYTES)) > MAX_STATE_BYTES
}

#[cfg(test)]
pub(super) fn ensure_admission_budget(doc: &DiskDoc) -> Result<(), DeckError> {
    if over_admission_budget(encode(doc)?.len(), doc) {
        return Err(DeckError::new(
            ErrorKind::DiskFull,
            "connector state capacity reached",
        ));
    }
    Ok(())
}

pub(super) fn validate_terminal(
    kind: &str,
    state: &str,
    code: Option<&str>,
    result: Option<&Value>,
) -> bool {
    if code.is_some_and(|value| {
        value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    }) || result.is_some_and(|value| {
        serde_json::to_vec(value)
            .map(|bytes| bytes.len() > MAX_RESULT_BYTES)
            .unwrap_or(true)
    }) {
        return false;
    }
    match state {
        "accepted" | "executing" => code.is_none() && result.is_none(),
        "rejected" | "ambiguous" => result.is_none(),
        "delivered" => kind == "send-message" && code.is_none() && result.is_none(),
        "applied" => {
            if matches!(kind, "queue-pause" | "queue-cancel") {
                return code.is_none() && result.is_none();
            }
            let Some(object) = result.and_then(Value::as_object) else {
                return false;
            };
            let id = |name: &str| {
                object
                    .get(name)
                    .and_then(Value::as_str)
                    .is_some_and(command_id)
            };
            let revision = || {
                object
                    .get("revision")
                    .and_then(Value::as_str)
                    .is_some_and(|value| {
                        !value.is_empty()
                            && value.len() <= 128
                            && !value.chars().any(char::is_control)
                    })
            };
            code.is_none()
                && match kind {
                    "task-create" => object.len() == 1 && id("cardId"),
                    "buffer-add" | "buffer-edit" | "buffer-delete" => {
                        object.len() == 3 && id("cardId") && id("entryId") && revision()
                    }
                    "buffer-queue" => {
                        object.len() == 3
                            && id("cardId")
                            && revision()
                            && object
                                .get("queued")
                                .and_then(Value::as_u64)
                                .is_some_and(|value| value <= 256)
                    }
                    _ => false,
                }
        }
        _ => false,
    }
}

impl Runtime {
    pub(super) fn pair(
        &self,
        epoch: u64,
        code: &str,
        device_name: &str,
    ) -> Result<Value, DeckError> {
        let device_name = crate::inbound_channel::strip_invisible(device_name)
            .trim()
            .to_owned();
        if device_name.is_empty()
            || device_name.chars().count() > 80
            || device_name.chars().any(char::is_control)
        {
            return Err(DeckError::new(ErrorKind::Invalid, "invalid device name"));
        }
        let _lifecycle = self.lifecycle.lock_or_recover();
        if !self.epoch_active(epoch) {
            return Err(DeckError::new(ErrorKind::Perm, "connector unavailable"));
        }
        let mut pairing = self.pairing.lock_or_recover();
        let p = pairing
            .as_ref()
            .ok_or_else(|| DeckError::new(ErrorKind::Perm, "pairing unavailable"))?;
        if p.expires_at <= now() || !bool::from(p.code.as_bytes().ct_eq(code.as_bytes())) {
            return Err(DeckError::new(ErrorKind::Perm, "pairing code invalid"));
        }
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random(32)?);
        let token_hash = sha(format!("deck-device-v1\0{token}").as_bytes());
        let device_id = random_id("device_", 16)?;
        let host_id = self.with_doc(|d| {
            if !d.config.enabled {
                return Err(DeckError::new(ErrorKind::Perm, "connector unavailable"));
            }
            if d.devices.len() >= MAX_DEVICES {
                prune_revoked_devices(d);
            }
            if d.devices.len() >= MAX_DEVICES {
                return Err(DeckError::new(
                    ErrorKind::DiskFull,
                    "device capacity reached",
                ));
            }
            d.devices.push(Device {
                id: device_id.clone(),
                name: device_name,
                token_hash,
                paired_at: now(),
                revoked_at: None,
                history_pruned: false,
                retired_through: 0,
            });
            Ok(d.host_id.clone())
        })?;
        *pairing = None;
        // The desktop hides the spent QR and names the new device at once.
        if let Some(app) = &self.app {
            let _ = app.emit("connector-changed", ());
        }
        Ok(json!({"version":1,"hostId":host_id,"deviceId":device_id,"token":token}))
    }
    pub(super) fn accept(
        &self,
        epoch: u64,
        device_id: &str,
        request: CommandRequest,
    ) -> Result<CommandResult, DeckError> {
        let _lifecycle = self.lifecycle.lock_or_recover();
        if !self.epoch_active(epoch) {
            return Err(DeckError::new(ErrorKind::Perm, "connector unavailable"));
        }
        validate_command(&request)?;
        let canonical = serde_json::to_vec(&request)
            .map_err(|_| DeckError::new(ErrorKind::Invalid, "invalid command"))?;
        let request_hash = sha(&canonical);
        let handle = sha(format!("{device_id}\0{}", request.id).as_bytes());
        let result = self.transact(true, |d| {
            if !d.config.enabled
                || !d
                    .devices
                    .iter()
                    .any(|device| device.id == device_id && device.revoked_at.is_none())
            {
                return Err(DeckError::new(ErrorKind::Perm, "unauthorized"));
            }
            if let Some(old) = d.commands.iter().find(|c| c.handle == handle) {
                if old.request_hash != request_hash {
                    return Err(DeckError::new(
                        ErrorKind::ContextChanged,
                        "command id reused with different body",
                    ));
                }
                return Ok(CommandResult {
                    id: old.id.clone(),
                    state: external_state(&old.state),
                    code: old.code.clone(),
                    result: old.result.clone(),
                });
            }
            // An id this host does not hold: its seq proves it was never
            // admitted before (see the module header). The check and the
            // insertion below are one persisted transaction.
            let Some(seq) = request.seq else {
                return Err(DeckError::new(ErrorKind::Invalid, CLIENT_UPGRADE_REQUIRED));
            };
            let floor = d
                .devices
                .iter()
                .find(|device| device.id == device_id)
                .map_or(0, |device| device.retired_through);
            if seq <= floor {
                return Err(DeckError::new(ErrorKind::ContextChanged, COMMAND_EXPIRED));
            }
            if d.commands
                .iter()
                .any(|c| c.device_id == device_id && c.seq == Some(seq))
            {
                return Err(DeckError::new(
                    ErrorKind::ContextChanged,
                    "command sequence reused with a different id",
                ));
            }
            if d.commands.iter().filter(|c| unresolved(&c.state)).count() >= MAX_COMMANDS {
                return Err(DeckError::new(
                    ErrorKind::DiskFull,
                    "command journal is full",
                ));
            }
            let at = now();
            d.commands.push(JournalEntry {
                handle,
                device_id: device_id.into(),
                request_hash,
                id: request.id.clone(),
                kind: request.kind.clone(),
                seq: Some(seq),
                request: Some(request.clone()),
                state: "accepted".into(),
                code: None,
                result: None,
                accepted_at: at,
                updated_at: at,
            });
            Ok(CommandResult {
                id: request.id,
                state: "accepted".into(),
                code: None,
                result: None,
            })
        })?;
        if let Some(app) = &self.app {
            let _ = app.emit("connector-changed", ());
        }
        Ok(result)
    }
    /// A tombstone answers with its original terminal result. Once any of a
    /// device's tombstones were dropped, an id its history no longer holds is
    /// `expired`: the host cannot prove it was never accepted, and `not-found`
    /// would invite the phone to retry (re-execute) it.
    pub(super) fn command_result(
        &self,
        device_id: &str,
        id: &str,
    ) -> Result<CommandResult, DeckError> {
        let handle = sha(format!("{device_id}\0{id}").as_bytes());
        self.read(|d| {
            if let Some(c) = d
                .commands
                .iter()
                .find(|c| c.handle == handle && c.device_id == device_id)
            {
                return Ok(CommandResult {
                    id: c.id.clone(),
                    state: external_state(&c.state),
                    code: c.code.clone(),
                    result: c.result.clone(),
                });
            }
            if d.devices
                .iter()
                .any(|device| device.id == device_id && device.history_pruned)
            {
                return Err(DeckError::new(ErrorKind::ContextChanged, COMMAND_EXPIRED));
            }
            Err(DeckError::new(ErrorKind::Missing, "command not found"))
        })?
    }
}

pub(super) const COMMAND_EXPIRED: &str = "command outcome expired";
/// A command without `seq` from a phone build that predates it.
pub(super) const CLIENT_UPGRADE_REQUIRED: &str = "connector client upgrade required";

/// Free device capacity held by revoked devices with no unresolved command.
/// Their tokens can no longer authenticate, so nothing can query or replay
/// their history; the record and its tombstones go together (every journal
/// entry must name an existing device).
pub(super) fn prune_revoked_devices(doc: &mut DiskDoc) {
    let busy = doc
        .commands
        .iter()
        .filter(|c| unresolved(&c.state))
        .map(|c| c.device_id.clone())
        .collect::<HashSet<_>>();
    let removed = doc
        .devices
        .iter()
        .filter(|d| d.revoked_at.is_some() && !busy.contains(&d.id))
        .map(|d| d.id.clone())
        .collect::<HashSet<_>>();
    doc.devices.retain(|d| !removed.contains(&d.id));
    doc.commands.retain(|c| !removed.contains(&c.device_id));
}
