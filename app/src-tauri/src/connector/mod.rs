//! Private-network-only authenticated Connector host.
//!
//! The listener is disabled by default and exposes only the closed v1 HTTPS
//! routes. TLS identity is Keychain-only; the private durable file contains
//! device token hashes, configuration and the command journal. Board
//! mutations remain webview-owned and are handed over through an opaque
//! journal handle after accepted intent is durable.
//!
//! Journal (file format v3; v1/v2 are upgraded on load and persisted as v3
//! on the next write, and an older reader refuses v3): only unresolved
//! (accepted/executing) entries carry their request, capped at
//! `MAX_COMMANDS`. Every committed write compacts resolved entries to
//! tombstones (id, kind, seq, request hash, state, code, result) so an exact
//! replay or recovery query still gets the original answer and a reused id
//! with a different body is still refused. At most `MAX_TOMBSTONES` are kept;
//! dropping one marks its device `history_pruned` (an unknown id is then
//! `expired` (410) on GET, never `not-found` (404), because a phone retries
//! a proven-absent id) and raises the device's `retired_through` to the
//! dropped command's `seq`.
//!
//! Request identity: (device, id) plus the canonical body hash, and a
//! device-assigned `seq` that the phone persists with the command before its
//! first POST and reuses on every retry. Admission (`accept`, the one path
//! behind POST /v1/commands) answers a known id from its record; otherwise it
//! requires a `seq` (426 `upgrade-required` without one), refuses a `seq` at
//! or below `retired_through` as `expired` (410) and a `seq` already held by
//! another retained id (409) — so a command whose tombstone was dropped is
//! never admitted again, whether or not the phone asked GET first, while any
//! higher `seq` keeps working. Revocation drops the device's tombstones; a
//! revoked device without unresolved work is pruned when a pairing needs its
//! slot. Each write encodes once and is atomic (temp file, file fsync,
//! rename, directory fsync).
//!
//! Phone reach: every card-scoped route requires a saved `codex`/`claude`
//! command with at most simple shell-safe arguments under the shared channel policy. Send-message and output also
//! require a live agent foreground process; output rechecks that identity
//! after capture alongside the generation/card checks. A foreground agent in
//! a plain shell card does not qualify. The snapshot exposes only those saved
//! agent cards and queue items belonging to them. Phone text is an agent
//! prompt, not a shell line, but a prompt can still lead the agent to run
//! commands. A session under MCP control (`mcp::guard_terminal_input`) is
//! neither written (checked before delivery and before each tmux write) nor
//! read (checked before and after capture).
//!
//! Network: RFC1918 addresses are eligible on any interface, 100.64/10 only
//! on `utun*` and 169.254/16 only on `bridge*` (`connector_network_address`).
//! `connector_enable` records the interface carrying the chosen address; a
//! restart binds only while the address is on that interface. The running
//! listener keeps the (address, interface, netmask) it started with and stops
//! when a periodic `getifaddrs` recheck finds a different one (`server.rs`).
//! Pairing strips bidi, zero-width and tag characters from the supplied
//! device name before validation and persistence, then emits
//! `connector-changed` so the desktop can show it at once.
//!
//! Layout (one contract, one file per concern; every file starts with
//! `use super::*` and exposes its items `pub(super)`): this file holds the
//! limits, the disk document and its records, the runtime and its lock
//! helpers, and `spawn_connector`; `journal` load/upgrade, compaction,
//! encode/persist, the admission budget, accept/resolve and device pruning;
//! `identity` the Keychain-backed TLS identity; `network` address
//! eligibility, interface classes and the listener recheck; `validate` the
//! request shapes and admission board checks; `projection` the saved agent
//! cards, snapshot, buffer and bounded output; `native` the claimed-command
//! execution and its `Transport`; `commands` the local Tauri commands;
//! `smoke` the debug-only smoke hooks; `server` the HTTPS listener.

mod server;

mod commands;
mod identity;
mod journal;
mod native;
mod network;
mod projection;
mod smoke;
#[cfg(test)]
mod tests;
mod validate;

pub(crate) use commands::*;
use identity::*;
use journal::*;
use native::*;
use network::*;
use projection::*;
pub(crate) use smoke::*;
use validate::*;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use subtle::ConstantTimeEq;
use tauri::{AppHandle, Emitter, Manager};

use crate::datadir::now_epoch as now;
use crate::error::{DeckError, ErrorKind};
use crate::keychain::{self, Slot};
use crate::ledger::{hex, random, random_id, sha};
use crate::scheduler::Queues;
use crate::sync::LockRecover;

/// v2 compacts terminal journal entries to tombstones (no request body) and
/// may record the listener interface. v3 adds the device admission floor
/// (`retiredThrough`) and each entry's `seq`; a v2 reader would drop the
/// floor and re-admit retired commands, so it refuses v3 untouched. Older
/// files are upgraded in memory and persisted as v3 on the next write.
const VERSION: u32 = 3;
const MAX_DEVICES: usize = 32;
/// Unresolved (accepted/executing) commands: the only entries that carry a
/// request body.
const MAX_COMMANDS: usize = 2000;
/// Terminal tombstones kept for exact replay and recovery answers. The oldest
/// is dropped first and its device is marked, so an id the host no longer
/// knows is answered `expired`, never `not-found` (which a phone may retry).
const MAX_TOMBSTONES: usize = 4000;
const MAX_STATE_BYTES: usize = 16 * 1024 * 1024;
const TERMINAL_RESERVE_BYTES: usize = 2 * 1024;
const MAX_RESULT_BYTES: usize = 1024;
const PAIR_TTL: u64 = 300;
const MAX_TEXT: usize = 32 * 1024;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Config {
    enabled: bool,
    address: String,
    port: u16,
    /// Interface that carried `address` when the user enabled the listener.
    /// A restart binds only while the address is still on that interface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interface: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Device {
    id: String,
    name: String,
    token_hash: String,
    paired_at: u64,
    revoked_at: Option<u64>,
    /// Some of this device's tombstones were dropped at capacity, so an id the
    /// host no longer knows cannot be proven never-accepted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    history_pruned: bool,
    /// Highest `seq` among this device's dropped tombstones: a new command
    /// must carry a higher one.
    #[serde(default, skip_serializing_if = "is_zero")]
    retired_through: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CommandRequest {
    pub(crate) id: String,
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) card_id: Option<String>,
    #[serde(default, skip_serializing_if = "ExpectedGeneration::is_missing")]
    pub(crate) expected_generation: ExpectedGeneration,
    #[serde(default)]
    pub(crate) expected_revision: Option<String>,
    pub(crate) payload: Value,
    /// Device-assigned admission sequence (see the module header). Optional
    /// on the wire only so an older phone gets `upgrade-required` instead of
    /// a parse failure, and so pre-v3 pending entries still load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) seq: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum ExpectedGeneration {
    #[default]
    Missing,
    Stopped,
    Live(String),
}

impl ExpectedGeneration {
    fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

impl Serialize for ExpectedGeneration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Missing | Self::Stopped => serializer.serialize_none(),
            Self::Live(value) => serializer.serialize_str(value),
        }
    }
}

impl<'de> Deserialize<'de> for ExpectedGeneration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Option::<String>::deserialize(deserializer)
            .map(|value| value.map(Self::Live).unwrap_or(Self::Stopped))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JournalEntry {
    handle: String,
    device_id: String,
    request_hash: String,
    /// Immutable device command id and kind; kept after compaction.
    #[serde(default)]
    id: String,
    #[serde(default)]
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seq: Option<u64>,
    /// Present exactly while the command is unresolved (accepted/executing);
    /// a terminal entry is a tombstone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request: Option<CommandRequest>,
    state: String,
    code: Option<String>,
    result: Option<Value>,
    accepted_at: u64,
    updated_at: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiskDoc {
    version: u32,
    host_id: String,
    config: Config,
    #[serde(default)]
    identity_address: Option<String>,
    #[serde(default)]
    identity_fingerprint: Option<String>,
    devices: Vec<Device>,
    commands: Vec<JournalEntry>,
}

impl DiskDoc {
    fn fresh() -> Result<Self, DeckError> {
        Ok(Self {
            version: VERSION,
            host_id: random_id("host_", 16)?,
            config: Config::default(),
            identity_address: None,
            identity_fingerprint: None,
            devices: vec![],
            commands: vec![],
        })
    }
}

struct Pairing {
    code: String,
    expires_at: u64,
}
struct Runtime {
    app: Option<AppHandle>,
    path: PathBuf,
    doc: Mutex<Result<DiskDoc, DeckError>>,
    pairing: Mutex<Option<Pairing>>,
    lifecycle: Mutex<()>,
    server_epoch: AtomicU64,
    running_epoch: AtomicU64,
}
static RUNTIME: OnceLock<Arc<Runtime>> = OnceLock::new();

impl Runtime {
    fn with_doc<T>(
        &self,
        f: impl FnOnce(&mut DiskDoc) -> Result<T, DeckError>,
    ) -> Result<T, DeckError> {
        self.transact(false, f)
    }
    /// One committed mutation: the candidate is compacted, encoded once and
    /// written durably before it replaces the in-memory document. The clone
    /// is the rollback of a failed write; compaction keeps it small (request
    /// bodies exist only for unresolved commands).
    fn transact<T>(
        &self,
        admission: bool,
        f: impl FnOnce(&mut DiskDoc) -> Result<T, DeckError>,
    ) -> Result<T, DeckError> {
        let mut guard = self.doc.lock_or_recover();
        let doc = guard.as_mut().map_err(|e| e.clone())?;
        let mut next = doc.clone();
        let out = f(&mut next)?;
        compact(&mut next);
        persist(&self.path, &next, admission)?;
        *doc = next;
        Ok(out)
    }
    fn read<T>(&self, f: impl FnOnce(&DiskDoc) -> T) -> Result<T, DeckError> {
        let g = self.doc.lock_or_recover();
        Ok(f(g.as_ref().map_err(|e| e.clone())?))
    }
    fn active_device(&self, token: &str) -> Option<String> {
        let digest = sha(format!("deck-device-v1\0{token}").as_bytes());
        self.read(|d| {
            d.devices
                .iter()
                .find(|x| {
                    x.revoked_at.is_none()
                        && bool::from(x.token_hash.as_bytes().ct_eq(digest.as_bytes()))
                })
                .map(|x| x.id.clone())
        })
        .ok()
        .flatten()
    }

    fn epoch_active(&self, epoch: u64) -> bool {
        epoch != 0
            && self.server_epoch.load(Ordering::SeqCst) == epoch
            && self.running_epoch.load(Ordering::SeqCst) == epoch
            && self.read(|d| d.config.enabled).unwrap_or(false)
    }

    fn feature_active(&self) -> bool {
        let epoch = self.server_epoch.load(Ordering::SeqCst);
        self.epoch_active(epoch)
    }

    fn authorize(&self, epoch: u64, token: &str) -> Result<String, DeckError> {
        if !self.epoch_active(epoch) {
            return Err(DeckError::new(ErrorKind::Perm, "connector unavailable"));
        }
        self.active_device(token)
            .ok_or_else(|| DeckError::new(ErrorKind::Perm, "unauthorized"))
    }

    fn executing(&self, handle: &str) -> Result<ExecutingCommand, DeckError> {
        self.read(|d| {
            let c = d
                .commands
                .iter()
                .find(|c| c.handle == handle)
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "command not found"))?;
            if c.state != "executing" {
                return Err(DeckError::new(ErrorKind::Other, "command is not executing"));
            }
            if !d
                .devices
                .iter()
                .any(|x| x.id == c.device_id && x.revoked_at.is_none())
            {
                return Err(DeckError::new(ErrorKind::Perm, "device revoked"));
            }
            Ok(ExecutingCommand {
                device_id: c.device_id.clone(),
                request: c.body()?.clone(),
            })
        })?
    }
}

impl JournalEntry {
    /// The request of an unresolved command (a tombstone has none).
    fn body(&self) -> Result<&CommandRequest, DeckError> {
        self.request
            .as_ref()
            .ok_or_else(|| DeckError::new(ErrorKind::Other, "command is not pending"))
    }
}

fn rt() -> Result<&'static Arc<Runtime>, DeckError> {
    RUNTIME
        .get()
        .ok_or_else(|| DeckError::new(ErrorKind::Other, "connector is not initialized"))
}

pub(crate) fn spawn_connector(app: AppHandle) {
    let path = crate::datadir::deck_dir().join("connector.json");
    let runtime = Arc::new(Runtime {
        app: Some(app.clone()),
        path: path.clone(),
        doc: Mutex::new(load(&path)),
        pairing: Mutex::new(None),
        lifecycle: Mutex::new(()),
        server_epoch: AtomicU64::new(0),
        running_epoch: AtomicU64::new(0),
    });
    let _ = RUNTIME.set(runtime.clone());
    if runtime.read(|d| d.config.enabled).unwrap_or(false) {
        let _ = start_server(runtime);
    }
    let wake = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(15));
        if connector_pending().map(|v| !v.is_empty()).unwrap_or(false) {
            let _ = wake.emit("connector-changed", ());
        }
    });
}

fn start_server(runtime: Arc<Runtime>) -> Result<(), DeckError> {
    let cfg = runtime.read(|d| d.config.clone())?;
    if !cfg.enabled {
        return Ok(());
    }
    let network = listener_network_ok(&cfg, &local_ipv4_interfaces())?;
    let identity = identity_get()?
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "connector identity is missing"))?;
    if identity.address != cfg.address {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "connector identity reset required",
        ));
    }
    let epoch = runtime.server_epoch.fetch_add(1, Ordering::SeqCst) + 1;
    let listening = cfg.clone();
    server::spawn(runtime, cfg, identity, epoch, move || {
        listener_network_unchanged(&listening, &network)
    })
    .map(|_| ())
}
