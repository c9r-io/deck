//! LAN-only authenticated Connector host.
//!
//! The listener is disabled by default and exposes only the closed v1 HTTPS
//! routes. TLS identity is Keychain-only; the private durable file contains
//! device token hashes, configuration and an append-only command-id ledger.
//! Board mutations remain webview-owned and are handed over through an opaque
//! journal handle after accepted intent is durable.

mod server;

use base64::Engine;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{DeckError, ErrorKind};
use crate::keychain::{self, Slot};
use crate::scheduler::Queues;
use crate::sync::LockRecover;

const VERSION: u32 = 1;
const MAX_DEVICES: usize = 32;
const MAX_COMMANDS: usize = 2000;
const MAX_STATE_BYTES: usize = 16 * 1024 * 1024;
const TERMINAL_RESERVE_BYTES: usize = 2 * 1024;
const MAX_RESULT_BYTES: usize = 1024;
const PAIR_TTL: u64 = 300;
const MAX_TEXT: usize = 32 * 1024;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
fn random(bytes: usize) -> Result<Vec<u8>, DeckError> {
    let mut out = vec![0; bytes];
    SystemRandom::new()
        .fill(&mut out)
        .map_err(|_| DeckError::new(ErrorKind::Other, "secure random unavailable"))?;
    Ok(out)
}
fn random_id(prefix: &str, bytes: usize) -> Result<String, DeckError> {
    Ok(format!("{prefix}{}", hex(&random(bytes)?)))
}
fn external_state(state: &str) -> String {
    if state == "executing" {
        "accepted".into()
    } else {
        state.into()
    }
}

fn invalidate_commands(doc: &mut DiskDoc, device_id: Option<&str>, code: &str) {
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

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Config {
    enabled: bool,
    address: String,
    port: u16,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Device {
    id: String,
    name: String,
    token_hash: String,
    paired_at: u64,
    revoked_at: Option<u64>,
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
    request: CommandRequest,
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

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Identity {
    address: String,
    cert_der: String,
    key_der: String,
    fingerprint: String,
}

impl Identity {
    fn generate(address: &str) -> Result<Self, DeckError> {
        let ip: IpAddr = address
            .parse()
            .map_err(|_| DeckError::new(ErrorKind::Invalid, "invalid connector address"))?;
        let mut params = rcgen::CertificateParams::default();
        params.subject_alt_names = vec![rcgen::SanType::IpAddress(ip)];
        let key = rcgen::KeyPair::generate()
            .map_err(|_| DeckError::new(ErrorKind::Other, "certificate generation failed"))?;
        let cert = params
            .self_signed(&key)
            .map_err(|_| DeckError::new(ErrorKind::Other, "certificate generation failed"))?;
        let cert_der = cert.der().to_vec();
        Ok(Self {
            address: address.into(),
            fingerprint: sha(&cert_der),
            cert_der: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(cert_der),
            key_der: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.serialize_der()),
        })
    }
    fn encode(&self) -> Result<String, DeckError> {
        Ok(format!(
            "v1_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(self)
                    .map_err(|_| DeckError::new(ErrorKind::Other, "identity encoding failed"))?
            )
        ))
    }
    fn decode(raw: &str) -> Result<Self, DeckError> {
        let raw = raw
            .strip_prefix("v1_")
            .ok_or_else(|| DeckError::new(ErrorKind::Recovery, "connector identity is invalid"))?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(raw)
            .map_err(|_| DeckError::new(ErrorKind::Recovery, "connector identity is invalid"))?;
        serde_json::from_slice(&bytes)
            .map_err(|_| DeckError::new(ErrorKind::Recovery, "connector identity is invalid"))
    }
    fn tls(&self) -> Result<rustls::ServerConfig, DeckError> {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        let cert = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&self.cert_der)
            .map_err(|_| DeckError::new(ErrorKind::Recovery, "connector identity is invalid"))?;
        let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&self.key_der)
            .map_err(|_| DeckError::new(ErrorKind::Recovery, "connector identity is invalid"))?;
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(cert)],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
            )
            .map_err(|_| DeckError::new(ErrorKind::Recovery, "connector identity is invalid"))
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

fn load(path: &Path) -> Result<DiskDoc, DeckError> {
    let bytes = match std::fs::File::open(path) {
        Ok(file) => {
            let mut v = Vec::new();
            file.take(MAX_STATE_BYTES as u64 + 1)
                .read_to_end(&mut v)
                .map_err(|e| {
                    DeckError::new(ErrorKind::io(e.kind()), "connector state could not be read")
                })?;
            v
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return DiskDoc::fresh(),
        Err(e) => {
            return Err(DeckError::new(
                ErrorKind::io(e.kind()),
                "connector state could not be read",
            ))
        }
    };
    if bytes.len() > MAX_STATE_BYTES {
        return Err(DeckError::new(
            ErrorKind::Recovery,
            "connector state exceeds its bounds",
        ));
    }
    let mut doc: DiskDoc = serde_json::from_slice(&bytes)
        .map_err(|_| DeckError::new(ErrorKind::Recovery, "connector state is unreadable"))?;
    if doc.version != VERSION
        || doc.devices.len() > MAX_DEVICES
        || doc.commands.len() > MAX_COMMANDS
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
    {
        return Err(DeckError::new(
            ErrorKind::Recovery,
            "connector state is invalid",
        ));
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
        let canonical = serde_json::to_vec(&c.request).ok();
        !handles.insert(&c.handle)
            || !device_ids.contains(&c.device_id)
            || c.handle != sha(format!("{}\0{}", c.device_id, c.request.id).as_bytes())
            || canonical
                .as_deref()
                .is_none_or(|bytes| c.request_hash != sha(bytes))
            || validate_command(&c.request).is_err()
            || !validate_terminal(&c.request, &c.state, c.code.as_deref(), c.result.as_ref())
    }) {
        return Err(DeckError::new(
            ErrorKind::Recovery,
            "connector journal records are invalid",
        ));
    }
    ensure_admission_budget(&doc).map_err(|_| {
        DeckError::new(
            ErrorKind::Recovery,
            "connector state exceeds its reserved capacity",
        )
    })?;
    let mut changed = false;
    for c in &mut doc.commands {
        if c.state == "executing" {
            c.state = "ambiguous".into();
            c.code = Some("interrupted".into());
            c.updated_at = now();
            changed = true;
        }
    }
    if changed {
        save(path, &doc)?;
    }
    Ok(doc)
}
fn save(path: &Path, doc: &DiskDoc) -> Result<(), DeckError> {
    let bytes = serde_json::to_vec(doc)
        .map_err(|_| DeckError::new(ErrorKind::Other, "connector state encoding failed"))?;
    if bytes.len() > MAX_STATE_BYTES {
        return Err(DeckError::new(
            ErrorKind::DiskFull,
            "connector state capacity reached",
        ));
    }
    crate::datadir::atomic_write(path, &bytes)
}

fn ensure_admission_budget(doc: &DiskDoc) -> Result<(), DeckError> {
    let encoded = serde_json::to_vec(doc)
        .map_err(|_| DeckError::new(ErrorKind::Other, "connector state encoding failed"))?;
    let unresolved = doc
        .commands
        .iter()
        .filter(|command| matches!(command.state.as_str(), "accepted" | "executing"))
        .count();
    if encoded
        .len()
        .saturating_add(unresolved.saturating_mul(TERMINAL_RESERVE_BYTES))
        > MAX_STATE_BYTES
    {
        return Err(DeckError::new(
            ErrorKind::DiskFull,
            "connector state capacity reached",
        ));
    }
    Ok(())
}

fn validate_terminal(
    request: &CommandRequest,
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
        "delivered" => request.kind == "send-message" && code.is_none() && result.is_none(),
        "applied" => {
            if matches!(request.kind.as_str(), "queue-pause" | "queue-cancel") {
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
                && match request.kind.as_str() {
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
    fn with_doc<T>(
        &self,
        f: impl FnOnce(&mut DiskDoc) -> Result<T, DeckError>,
    ) -> Result<T, DeckError> {
        let mut guard = self.doc.lock_or_recover();
        let doc = guard.as_mut().map_err(|e| e.clone())?;
        let mut next = doc.clone();
        let out = f(&mut next)?;
        save(&self.path, &next)?;
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
                request: c.request.clone(),
            })
        })?
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

fn identity_get() -> Result<Option<Identity>, DeckError> {
    keychain::get_checked(Slot::ConnectorIdentity)?
        .map(|v| Identity::decode(&v))
        .transpose()
}
fn start_server(runtime: Arc<Runtime>) -> Result<(), DeckError> {
    let cfg = runtime.read(|d| d.config.clone())?;
    if !cfg.enabled {
        return Ok(());
    }
    let identity = identity_get()?
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "connector identity is missing"))?;
    if identity.address != cfg.address {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "connector identity reset required",
        ));
    }
    let epoch = runtime.server_epoch.fetch_add(1, Ordering::SeqCst) + 1;
    server::spawn(runtime, cfg, identity, epoch).map(|_| ())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    enabled: bool,
    running: bool,
    address: String,
    port: u16,
    origin: Option<String>,
    fingerprint: Option<String>,
    reset_required: bool,
    devices: Vec<DeviceView>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceView {
    id: String,
    name: String,
    paired_at: u64,
    revoked: bool,
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
        let ip: Ipv4Addr = address
            .parse()
            .map_err(|_| DeckError::new(ErrorKind::Invalid, "invalid connector address"))?;
        if !local_ipv4_addresses().contains(&ip)
            || ip.is_unspecified()
            || ip.is_loopback()
            || port < 1024
        {
            return Err(DeckError::new(
                ErrorKind::Invalid,
                "connector address or port is invalid",
            ));
        }
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
        r.with_doc(|d| {
            d.config = Config {
                enabled: true,
                address: address.clone(),
                port,
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
    uri: String,
    svg: String,
    expires_at: u64,
    origin: String,
    fingerprint: String,
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
    let data = json!({"version":1,"hostId":host_id,"hostName":host_name(),"origin":origin,"fingerprint":identity.fingerprint,"code":code,"expiresAt":expires_at});
    let encoded =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&data).unwrap());
    if encoded.len() > 8 * 1024 {
        *r.pairing.lock_or_recover() = None;
        return Err(DeckError::new(
            ErrorKind::Other,
            "pairing descriptor is too large",
        ));
    }
    let uri = format!("deck-connector://pair?data={encoded}");
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
    rt()?.with_doc(|d| {
        let x = d
            .devices
            .iter_mut()
            .find(|x| x.id == device_id)
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "device not found"))?;
        x.revoked_at = Some(now());
        invalidate_commands(d, Some(&device_id), "device-revoked");
        Ok(())
    })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingView {
    handle: String,
    request: CommandRequest,
}
struct ExecutingCommand {
    device_id: String,
    request: CommandRequest,
}
#[tauri::command]
pub(crate) fn connector_pending() -> Result<Vec<PendingView>, DeckError> {
    rt()?.read(|d| {
        d.commands
            .iter()
            .filter(|c| c.state == "accepted")
            .map(|c| PendingView {
                handle: c.handle.clone(),
                request: c.request.clone(),
            })
            .collect()
    })
}

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
        };
        if !doc.devices.iter().any(|device| device.id == "device_smoke") {
            doc.devices.push(Device {
                id: "device_smoke".into(),
                name: "Smoke device".into(),
                token_hash: "a".repeat(64),
                paired_at: now(),
                revoked_at: None,
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
    };
    let handle = sha(format!("device_smoke\0{}", request.id).as_bytes());
    runtime.accept(epoch, "device_smoke", request.clone())?;
    Ok(PendingView { handle, request })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SmokeTransportView {
    path: String,
}

fn require_smoke_session_stopped(
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
            },
            identity,
            epoch,
        )?;
        if let Err(error) = runtime.with_doc(|doc| {
            doc.config = Config {
                enabled: true,
                address: "127.0.0.1".into(),
                port,
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
        c.state = "executing".into();
        c.updated_at = now();
        Ok(PendingView {
            handle: c.handle.clone(),
            request: c.request.clone(),
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
        if !validate_terminal(&c.request, &state, code.as_deref(), result.as_ref()) {
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
        Ok(c.request.clone())
    })??;
    validate_applicable(&request)?;
    Ok(true)
}

fn buffer_operation_id(handle: &str, entry_id: &str) -> String {
    let digest = Sha256::digest(format!("connector-buffer:{handle}:{entry_id}").as_bytes());
    format!("B{}", hex(&digest[..16]))
}

fn validate_admission_board(
    handle: &str,
    request: &CommandRequest,
    board: &Value,
) -> Result<(), DeckError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Payload {
        entry_ids: Vec<String>,
    }
    if request.kind != "buffer-queue" {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "command is not a buffer admission",
        ));
    }
    let payload: Payload = serde_json::from_value(request.payload.clone())
        .map_err(|_| DeckError::new(ErrorKind::Invalid, "command payload is invalid"))?;
    let card_id = request
        .card_id
        .as_deref()
        .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "card is required"))?;
    let card = board
        .get("cards")
        .and_then(Value::as_array)
        .and_then(|cards| {
            cards
                .iter()
                .find(|card| card.get("id").and_then(Value::as_str) == Some(card_id))
        })
        .ok_or_else(|| DeckError::new(ErrorKind::ContextChanged, "buffer-copies-missing"))?;
    require_queue_target(card)?;
    let entries = card
        .get("buffer")
        .and_then(|buffer| buffer.get("entries"))
        .and_then(Value::as_array)
        .ok_or_else(|| DeckError::new(ErrorKind::ContextChanged, "buffer-copies-missing"))?;
    for entry_id in payload.entry_ids {
        let entry = entries
            .iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(&entry_id))
            .ok_or_else(|| DeckError::new(ErrorKind::ContextChanged, "buffer-copies-missing"))?;
        let expected = buffer_operation_id(handle, &entry_id);
        let count = entry
            .get("copies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|copy| copy.get("operationId").and_then(Value::as_str) == Some(&expected))
            .count();
        if count != 1 {
            return Err(DeckError::new(
                ErrorKind::ContextChanged,
                "buffer-copies-missing",
            ));
        }
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn connector_validate_admission(handle: String) -> Result<bool, DeckError> {
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
        Ok(command.request.clone())
    })??;
    let (_, board) = board_value()?;
    validate_admission_board(&handle, &request, &board)?;
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

fn execute_native(
    req: &CommandRequest,
    handle: &str,
    device_id: &str,
    queues: &Queues,
) -> Result<&'static str, (&'static str, &'static str)> {
    match req.kind.as_str() {
        "send-message" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct P {
                text: String,
            }
            let p: P = serde_json::from_value(req.payload.clone())
                .map_err(|_| ("rejected", "invalid-payload"))?;
            if p.text.is_empty()
                || p.text.len() > MAX_TEXT
                || p.text
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
            {
                return Err(("rejected", "invalid-text"));
            }
            let card = committed_card(req.card_id.as_deref().ok_or(("rejected", "missing-card"))?)
                .map_err(|_| ("rejected", "missing-card"))?;
            let probe = crate::context::connector_probe(&card.session)
                .map_err(|_| ("rejected", "target-changed"))?;
            if !matches!(
                &req.expected_generation,
                ExpectedGeneration::Live(expected) if expected == &probe.generation
            ) {
                return Err(("rejected", "target-changed"));
            }
            let agent = probe
                .agent
                .as_deref()
                .ok_or(("rejected", "agent-not-ready"))?;
            if !crate::scheduler::claim_session(&queues.busy, &card.session) {
                return Err(("rejected", "session-busy"));
            }
            let _busy = BusyClaim {
                busy: &queues.busy,
                session: &card.session,
            };
            let transport = ConnectorTransport {
                card_id: &card.id,
                session: &card.session,
                expected_generation: &probe.generation,
                device_id,
            };
            let outcome = crate::prompt_delivery::deliver_with(
                crate::prompt_delivery::LiteralRequest {
                    session: &card.session,
                    pane: &probe.identity,
                    expected_process: Some(agent),
                    delivery: &handle[..handle.len().min(48)],
                    text: &p.text,
                    submit: true,
                    require_bracketed: true,
                },
                &transport,
            );
            match outcome {
                Ok(crate::prompt_delivery::LiteralOutcome::Submitted) => Ok("delivered"),
                Ok(_) => Err(("ambiguous", "enter-refused")),
                Err(e) if e.kind() == ErrorKind::ContextChanged => {
                    Err(("rejected", "target-changed"))
                }
                Err(_) => Err(("ambiguous", "delivery-unknown")),
            }
        }
        "queue-pause" | "queue-cancel" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct P {
                item_id: String,
                #[serde(default)]
                paused: Option<bool>,
                revision: String,
            }
            let p: P = serde_json::from_value(req.payload.clone())
                .map_err(|_| ("rejected", "invalid-payload"))?;
            let card = committed_card(req.card_id.as_deref().ok_or(("rejected", "missing-card"))?)
                .map_err(|_| ("rejected", "missing-card"))?;
            let rev = p
                .revision
                .parse()
                .map_err(|_| ("rejected", "revision-changed"))?;
            crate::scheduler::connector::mutate(
                queues,
                &card.id,
                &card.session,
                &p.item_id,
                rev,
                if req.kind == "queue-pause" {
                    Some(p.paused.ok_or(("rejected", "invalid-payload"))?)
                } else {
                    None
                },
                || {
                    let runtime = rt()?;
                    if !runtime.feature_active() {
                        return Err(DeckError::new(ErrorKind::Perm, "connector-disabled"));
                    }
                    let active = runtime.read(|d| {
                        d.devices
                            .iter()
                            .any(|x| x.id == device_id && x.revoked_at.is_none())
                    })?;
                    if !active {
                        return Err(DeckError::new(ErrorKind::Perm, "device-revoked"));
                    }
                    match crate::context::connector_probe(&card.session) {
                        Ok(probe)
                            if matches!(
                                &req.expected_generation,
                                ExpectedGeneration::Live(expected)
                                    if expected == &probe.generation
                            ) =>
                        {
                            Ok(())
                        }
                        Ok(_) => Err(DeckError::new(ErrorKind::ContextChanged, "target-changed")),
                        Err(e)
                            if matches!(&req.expected_generation, ExpectedGeneration::Stopped)
                                && e.kind() == ErrorKind::NoSession =>
                        {
                            Ok(())
                        }
                        Err(_) => Err(DeckError::new(ErrorKind::Other, "target-unknown")),
                    }
                },
            )
            .map_err(|e| {
                (
                    "rejected",
                    match e.message() {
                        "target-changed" => "target-changed",
                        "target-unknown" => "target-unknown",
                        "device-revoked" => "device-revoked",
                        "revision-changed" => "revision-changed",
                        _ => "queue-conflict",
                    },
                )
            })?;
            Ok("applied")
        }
        _ => Err(("rejected", "frontend-required")),
    }
}

struct BusyClaim<'a> {
    busy: &'a Mutex<HashSet<String>>,
    session: &'a str,
}

impl Drop for BusyClaim<'_> {
    fn drop(&mut self) {
        crate::scheduler::release_session(self.busy, self.session);
    }
}

struct ConnectorTransport<'a> {
    card_id: &'a str,
    session: &'a str,
    expected_generation: &'a str,
    device_id: &'a str,
}

impl ConnectorTransport<'_> {
    fn guard(&self) -> Result<(), DeckError> {
        let runtime = rt()?;
        if !runtime.feature_active() {
            return Err(DeckError::new(ErrorKind::Perm, "connector unavailable"));
        }
        if committed_card(self.card_id)?.session != self.session {
            return Err(DeckError::new(ErrorKind::ContextChanged, "target-changed"));
        }
        let device_active = runtime.read(|d| {
            d.devices
                .iter()
                .any(|x| x.id == self.device_id && x.revoked_at.is_none())
        })?;
        if !device_active {
            return Err(DeckError::new(ErrorKind::Perm, "device revoked"));
        }
        let generation = crate::context::connector_probe(self.session)?.generation;
        if generation != self.expected_generation {
            return Err(DeckError::new(ErrorKind::ContextChanged, "target-changed"));
        }
        Ok(())
    }
}

impl crate::prompt_delivery::Transport for ConnectorTransport<'_> {
    fn probe(&self, session: &str) -> Result<crate::context::RawProbe, DeckError> {
        crate::prompt_delivery::Transport::probe(&crate::prompt_delivery::TmuxTransport, session)
    }

    fn run(&self, args: &[String]) -> Result<String, DeckError> {
        // Cleanup must remain possible after a target or authorization change.
        if args.first().map(String::as_str) != Some("delete-buffer") {
            self.guard()?;
        }
        crate::prompt_delivery::Transport::run(&crate::prompt_delivery::TmuxTransport, args)
    }

    fn run_with_stdin(&self, args: &[String], input: &[u8]) -> Result<String, DeckError> {
        self.guard()?;
        crate::prompt_delivery::Transport::run_with_stdin(
            &crate::prompt_delivery::TmuxTransport,
            args,
            input,
        )
    }

    fn pause(&self, duration: std::time::Duration) {
        crate::prompt_delivery::Transport::pause(&crate::prompt_delivery::TmuxTransport, duration)
    }
}

#[derive(Clone)]
struct InternalCard {
    id: String,
    session: String,
}
fn board_value() -> Result<(String, Value), DeckError> {
    let raw = crate::documents::connector_board_payload()?;
    let rev = sha(raw.as_bytes());
    let value = serde_json::from_str(&raw)
        .map_err(|_| DeckError::new(ErrorKind::Recovery, "board projection failed"))?;
    Ok((rev, value))
}
fn committed_card(id: &str) -> Result<InternalCard, DeckError> {
    let (_, v) = board_value()?;
    let c = v
        .get("cards")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .find(|c| c.get("id").and_then(Value::as_str) == Some(id))
        })
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "card not found"))?;
    Ok(InternalCard {
        id: id.into(),
        session: c
            .get("session")
            .and_then(Value::as_str)
            .ok_or_else(|| DeckError::new(ErrorKind::InvalidDoc, "card session missing"))?
            .into(),
    })
}

fn queue_target_supported(card: &Value) -> bool {
    card.get("cmd")
        .and_then(Value::as_str)
        .and_then(crate::context::expected_from_command)
        .is_some_and(|command| matches!(command.as_str(), "codex" | "claude"))
}

fn require_queue_target(card: &Value) -> Result<(), DeckError> {
    if queue_target_supported(card) {
        Ok(())
    } else {
        Err(DeckError::new(ErrorKind::Invalid, "unsupported-target"))
    }
}

fn validate_applicable(request: &CommandRequest) -> Result<(), DeckError> {
    let (revision, board) = board_value()?;
    if request.kind == "task-create" {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct P {
            project_id: String,
            preset_id: String,
        }
        let p: P = serde_json::from_value(request.payload.clone())
            .map_err(|_| DeckError::new(ErrorKind::Invalid, "invalid task preset"))?;
        if request.expected_revision.as_deref() != Some(&revision) {
            return Err(DeckError::new(
                ErrorKind::ContextChanged,
                "revision-changed",
            ));
        }
        let project = board
            .get("projects")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter()
                    .find(|x| x.get("id").and_then(Value::as_str) == Some(&p.project_id))
            })
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "task project not found"))?;
        let presets = project
            .get("presets")
            .and_then(Value::as_array)
            .filter(|a| a.len() <= 50)
            .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "task presets are invalid"))?;
        let preset = presets
            .iter()
            .find(|x| x.get("id").and_then(Value::as_str) == Some(&p.preset_id))
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "task preset not found"))?;
        let bounded = |field: &str, max: usize| {
            preset
                .get(field)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty() && v.len() <= max && !v.chars().any(char::is_control))
        };
        let column_id = bounded("columnId", 128)
            .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "task preset is invalid"))?;
        if bounded("id", 128).is_none()
            || bounded("name", 120).is_none()
            || bounded("title", 120).is_none()
            || bounded("dir", 1024).is_none()
            || project
                .get("columns")
                .and_then(Value::as_array)
                .is_none_or(|columns| {
                    !columns
                        .iter()
                        .any(|c| c.get("id").and_then(Value::as_str) == Some(column_id))
                })
        {
            return Err(DeckError::new(ErrorKind::Invalid, "task preset is invalid"));
        }
        let command = bounded("cmd", 200).and_then(crate::context::expected_from_command);
        if !command
            .as_deref()
            .is_some_and(|x| matches!(x, "codex" | "claude"))
        {
            return Err(DeckError::new(
                ErrorKind::Invalid,
                "task preset command is not supported",
            ));
        }
        let steps = preset
            .get("steps")
            .and_then(Value::as_array)
            .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "task preset steps are invalid"))?;
        if steps.len() > 20
            || steps
                .iter()
                .any(|x| x.as_str().is_none_or(|s| s.is_empty() || s.len() > 2000))
        {
            return Err(DeckError::new(
                ErrorKind::Invalid,
                "task preset steps are invalid",
            ));
        }
        return Ok(());
    }
    if request.kind.starts_with("buffer-") {
        let id = request
            .card_id
            .as_deref()
            .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "card is required"))?;
        let card = board
            .get("cards")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter()
                    .find(|c| c.get("id").and_then(Value::as_str) == Some(id))
            })
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "card not found"))?;
        if request.kind == "buffer-queue" {
            require_queue_target(card)?;
        }
        let current = card
            .get("buffer")
            .and_then(|b| b.get("revision"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .to_string();
        if request.expected_revision.as_deref() != Some(&current) {
            return Err(DeckError::new(
                ErrorKind::ContextChanged,
                "revision-changed",
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CommandResult {
    id: String,
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
}

fn validate_command(r: &CommandRequest) -> Result<(), DeckError> {
    if r.id.is_empty() || r.id.len() > 128 || r.id.chars().any(|c| c.is_control()) {
        return Err(DeckError::new(ErrorKind::Invalid, "invalid command id"));
    }
    if !matches!(
        r.kind.as_str(),
        "send-message"
            | "buffer-add"
            | "buffer-edit"
            | "buffer-delete"
            | "buffer-queue"
            | "task-create"
            | "queue-pause"
            | "queue-cancel"
    ) {
        return Err(DeckError::new(ErrorKind::Invalid, "unknown command kind"));
    }
    if r.kind != "task-create"
        && r.card_id
            .as_ref()
            .is_none_or(|v| v.is_empty() || v.len() > 128)
    {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "command card is invalid",
        ));
    }
    if r.kind == "send-message"
        && !matches!(
            &r.expected_generation,
            ExpectedGeneration::Live(v)
                if v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit())
        )
    {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "expected generation is required",
        ));
    }
    if matches!(r.kind.as_str(), "queue-pause" | "queue-cancel") {
        match &r.expected_generation {
            ExpectedGeneration::Stopped => {}
            ExpectedGeneration::Live(v)
                if v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()) => {}
            _ => {
                return Err(DeckError::new(
                    ErrorKind::Invalid,
                    "expected generation is required",
                ));
            }
        }
    }
    if matches!(
        r.kind.as_str(),
        "buffer-add" | "buffer-edit" | "buffer-delete" | "buffer-queue" | "task-create"
    ) && r
        .expected_revision
        .as_ref()
        .is_none_or(|v| v.is_empty() || v.len() > 128)
    {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "expected revision is required",
        ));
    }
    validate_command_payload(r)?;
    Ok(())
}

fn command_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

fn command_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TEXT
        && !value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
}

fn validate_command_payload(r: &CommandRequest) -> Result<(), DeckError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Text {
        text: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Edit {
        entry_id: String,
        text: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Entry {
        entry_id: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Entries {
        entry_ids: Vec<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Task {
        project_id: String,
        preset_id: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Queue {
        item_id: String,
        #[serde(default)]
        paused: Option<bool>,
        revision: String,
    }

    let valid = match r.kind.as_str() {
        "send-message" | "buffer-add" => {
            serde_json::from_value::<Text>(r.payload.clone()).is_ok_and(|p| command_text(&p.text))
        }
        "buffer-edit" => serde_json::from_value::<Edit>(r.payload.clone())
            .is_ok_and(|p| command_id(&p.entry_id) && command_text(&p.text)),
        "buffer-delete" => serde_json::from_value::<Entry>(r.payload.clone())
            .is_ok_and(|p| command_id(&p.entry_id)),
        "buffer-queue" => serde_json::from_value::<Entries>(r.payload.clone()).is_ok_and(|p| {
            let unique = p.entry_ids.iter().collect::<HashSet<_>>().len() == p.entry_ids.len();
            !p.entry_ids.is_empty()
                && p.entry_ids.len() <= 256
                && unique
                && p.entry_ids.iter().all(|id| command_id(id))
        }),
        "task-create" => serde_json::from_value::<Task>(r.payload.clone())
            .is_ok_and(|p| command_id(&p.project_id) && command_id(&p.preset_id)),
        "queue-pause" | "queue-cancel" => serde_json::from_value::<Queue>(r.payload.clone())
            .is_ok_and(|p| {
                command_id(&p.item_id)
                    && !p.revision.is_empty()
                    && p.revision.len() <= 32
                    && p.revision.bytes().all(|b| b.is_ascii_digit())
                    && if r.kind == "queue-pause" {
                        p.paused.is_some()
                    } else {
                        p.paused.is_none()
                    }
            }),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(DeckError::new(
            ErrorKind::Invalid,
            "command payload is invalid",
        ))
    }
}

impl Runtime {
    pub(super) fn pair(
        &self,
        epoch: u64,
        code: &str,
        device_name: &str,
    ) -> Result<Value, DeckError> {
        if device_name.trim().is_empty()
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
                return Err(DeckError::new(
                    ErrorKind::DiskFull,
                    "device capacity reached",
                ));
            }
            d.devices.push(Device {
                id: device_id.clone(),
                name: device_name.trim().into(),
                token_hash,
                paired_at: now(),
                revoked_at: None,
            });
            Ok(d.host_id.clone())
        })?;
        *pairing = None;
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
        let result = self.with_doc(|d| {
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
                    id: old.request.id.clone(),
                    state: external_state(&old.state),
                    code: old.code.clone(),
                    result: old.result.clone(),
                });
            }
            if d.commands.len() >= MAX_COMMANDS {
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
                request: request.clone(),
                state: "accepted".into(),
                code: None,
                result: None,
                accepted_at: at,
                updated_at: at,
            });
            ensure_admission_budget(d)?;
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
    pub(super) fn command_result(
        &self,
        device_id: &str,
        id: &str,
    ) -> Result<CommandResult, DeckError> {
        let handle = sha(format!("{device_id}\0{id}").as_bytes());
        self.read(|d| {
            d.commands
                .iter()
                .find(|c| c.handle == handle && c.device_id == device_id)
                .map(|c| CommandResult {
                    id: c.request.id.clone(),
                    state: external_state(&c.state),
                    code: c.code.clone(),
                    result: c.result.clone(),
                })
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "command not found"))
        })?
    }
}

pub(super) fn snapshot(app: &AppHandle) -> Result<Value, DeckError> {
    let (revision, b) = board_value()?;
    let queue = app.state::<Queues>();
    let (_, items, _) = crate::scheduler::connector::snapshot(&queue);
    let projects = b.get("projects").and_then(Value::as_array).into_iter().flatten().filter_map(|p| {
        let columns = p.get("columns").and_then(Value::as_array).into_iter().flatten().filter_map(|c| Some(json!({"id":c.get("id")?.as_str()?,"name":c.get("name")?.as_str()?}))).collect::<Vec<_>>();
        let presets = p.get("presets").and_then(Value::as_array).into_iter().flatten().filter_map(|x| Some(json!({"id":x.get("id")?.as_str()?,"name":x.get("name")?.as_str()?}))).collect::<Vec<_>>();
        Some(json!({"id":p.get("id")?.as_str()?,"name":p.get("name")?.as_str()?,"columns":columns,"presets":presets}))
    }).collect::<Vec<_>>();
    let cards = b
        .get("cards")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let id = c.get("id")?.as_str()?;
            let session = c.get("session")?.as_str()?;
            let (status, probe) = probe_status(crate::context::connector_probe(session));
            let buffer = c.get("buffer");
            Some(json!({
                "id":id,
                "projectId":c.get("projectId")?.as_str()?,
                "columnId":c.get("columnId")?.as_str()?,
                "title":c.get("title")?.as_str()?,
                "status":status,
                "generation":probe.as_ref().map(|p|p.generation.clone()),
                "canSend":probe.as_ref().is_some_and(|p|p.agent.is_some()),
                "canQueue":queue_target_supported(c),
                "buffer":{
                    "revision":buffer.and_then(|v|v.get("revision")).and_then(Value::as_u64).unwrap_or(0),
                    "collecting":buffer.and_then(|v|v.get("collecting")).and_then(Value::as_bool).unwrap_or(false),
                    "entryCount":buffer.and_then(|v|v.get("entries")).and_then(Value::as_array).map(|a|a.len()).unwrap_or(0)
                }
            }))
        })
        .collect::<Vec<_>>();
    Ok(
        json!({"version":1,"hostId":rt()?.read(|d|d.host_id.clone())?,"revision":revision,"capturedAt":now(),"projects":projects,"cards":cards,"queue":items}),
    )
}

fn probe_status<T>(probe: Result<T, DeckError>) -> (&'static str, Option<T>) {
    match probe {
        Ok(value) => ("running", Some(value)),
        Err(error) if error.kind() == ErrorKind::NoSession => ("stopped", None),
        Err(_) => ("unknown", None),
    }
}

pub(super) fn buffer(app: &AppHandle, card_id: &str) -> Result<Value, DeckError> {
    let (_, b) = board_value()?;
    let c = b
        .get("cards")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .find(|c| c.get("id").and_then(Value::as_str) == Some(card_id))
        })
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "card not found"))?;
    let mut out = c
        .get("buffer")
        .cloned()
        .unwrap_or_else(|| json!({"revision":0,"collecting":false,"entries":[]}));
    let queues = app.state::<Queues>();
    let (_, _, ops) = crate::scheduler::connector::snapshot(&queues);
    if let Some(entries) = out.get_mut("entries").and_then(Value::as_array_mut) {
        for e in entries {
            if let Some(copies) = e.get_mut("copies").and_then(Value::as_array_mut) {
                for copy in copies {
                    if let Some(id) = copy.get("operationId").and_then(Value::as_str) {
                        if let Some(op) = ops.iter().find(|o| o.id == id) {
                            copy["state"] = Value::String(op.state.clone());
                        } else {
                            copy["state"] = Value::String("uncertain".into());
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

pub(super) fn output(card_id: &str) -> Result<Value, DeckError> {
    let card = committed_card(card_id)?;
    let before = crate::context::connector_probe(&card.session)?;
    let history_size = crate::tmux::tmux_owned(&[
        "display-message".into(),
        "-p".into(),
        "-t".into(),
        before.identity.pane_id.clone(),
        "#{history_size}".into(),
    ])?
    .trim()
    .parse::<usize>()
    .map_err(|_| DeckError::new(ErrorKind::Tmux, "output-history-unavailable"))?;
    let text = crate::tmux::tmux_owned(&[
        "capture-pane".into(),
        "-p".into(),
        "-t".into(),
        before.identity.pane_id.clone(),
        "-S".into(),
        "-200".into(),
    ])?;
    let after = crate::context::connector_probe(&card.session)?;
    if before.generation != after.generation || committed_card(card_id)?.session != card.session {
        return Err(DeckError::new(ErrorKind::ContextChanged, "target-changed"));
    }
    let bytes = text.as_bytes();
    let truncated = history_size > 200 || bytes.len() > 64 * 1024;
    let text = if truncated {
        let mut s = bytes.len() - 64 * 1024;
        while !text.is_char_boundary(s) {
            s += 1;
        }
        text[s..].to_string()
    } else {
        text
    };
    let revision = sha(format!("{}\0{text}", before.generation).as_bytes());
    Ok(
        json!({"cardId":card_id,"generation":before.generation,"revision":revision,"capturedAt":now(),"text":text,"truncated":truncated}),
    )
}

fn local_ipv4_addresses() -> Vec<Ipv4Addr> {
    unsafe {
        let mut head = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 {
            return vec![];
        }
        let mut out = vec![];
        let mut p = head;
        while !p.is_null() {
            let a = &*p;
            if !a.ifa_addr.is_null() && (*a.ifa_addr).sa_family as i32 == libc::AF_INET {
                let sin = &*(a.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                if !ip.is_unspecified() && !ip.is_loopback() {
                    out.push(ip);
                }
            }
            p = a.ifa_next;
        }
        libc::freeifaddrs(head);
        out.sort();
        out.dedup();
        out
    }
}
fn host_name() -> String {
    let mut b = [0i8; 256];
    unsafe {
        if libc::gethostname(b.as_mut_ptr(), b.len()) == 0 {
            let n = b.iter().position(|x| *x == 0).unwrap_or(b.len());
            return String::from_utf8_lossy(std::slice::from_raw_parts(b.as_ptr() as *const u8, n))
                .chars()
                .filter(|c| !c.is_control())
                .take(80)
                .collect();
        }
    }
    "Mac".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::{CertificateDer, ServerName};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    fn test_runtime(tag: &str) -> (Arc<Runtime>, tauri::App<tauri::test::MockRuntime>) {
        let app = tauri::test::mock_app();
        let path =
            std::env::temp_dir().join(format!("deck-connector-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut doc = DiskDoc::fresh().unwrap();
        doc.config = Config {
            enabled: true,
            address: "127.0.0.1".into(),
            port: 8443,
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
        }
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
            &request,
            "applied",
            None,
            Some(&json!({"cardId":"C1","entryId":"E1","revision":"2"}))
        ));
        assert!(!validate_terminal(
            &request,
            "applied",
            None,
            Some(&json!({"cardId":"C1","entryId":"E1","revision":"2","extra":true}))
        ));
        assert!(!validate_terminal(
            &request,
            "rejected",
            Some("UPPER_CASE"),
            None
        ));
        assert!(!validate_terminal(
            &request,
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
                });
                Ok(())
            })
            .unwrap();
        let escaped = "\n".repeat(MAX_TEXT);
        let accepted = runtime.accept(1, "D", request("exact", &escaped)).unwrap();
        assert_eq!(accepted.state, "accepted");
        assert_eq!(
            runtime
                .read(|doc| doc.commands[0].request.payload["text"]
                    .as_str()
                    .unwrap()
                    .len())
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
        });
        let make = |index: usize| {
            let request = request(&format!("I{index}"), &"x".repeat(MAX_TEXT));
            JournalEntry {
                handle: sha(format!("D\0{}", request.id).as_bytes()),
                device_id: "D".into(),
                request_hash: sha(&serde_json::to_vec(&request).unwrap()),
                request,
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
            &terminal.request,
            &terminal.state,
            None,
            terminal.result.as_ref()
        ));
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
        };
        let copy1 = buffer_operation_id(&handle, "E1");
        let copy2 = buffer_operation_id(&handle, "E2");
        let board = json!({"cards":[{"id":"C1","cmd":"codex --full-auto","buffer":{"revision":9,"entries":[
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
    fn buffer_queue_requires_saved_trusted_agent_command_in_both_phases() {
        assert!(queue_target_supported(&json!({"cmd":"codex --full-auto"})));
        assert!(queue_target_supported(
            &json!({"cmd":"env FOO=1 /opt/bin/claude --x"})
        ));
        for card in [
            json!({"cmd":""}),
            json!({"cmd":"/bin/zsh"}),
            json!({"cmd":"/bin/zsh -lc codex"}),
        ] {
            let error = require_queue_target(&card).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Invalid);
            assert_eq!(error.message(), "unsupported-target");
        }

        let handle = "a".repeat(64);
        let request = CommandRequest {
            id: "queue-guard".into(),
            kind: "buffer-queue".into(),
            card_id: Some("C1".into()),
            expected_generation: ExpectedGeneration::Missing,
            expected_revision: Some("1".into()),
            payload: json!({"entryIds":["E1"]}),
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
                require_smoke_session_stopped("target", Err(DeckError::new(kind, "absent")))
                    .is_ok()
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
            });
            d.devices.push(Device {
                id: "D2".into(),
                name: "two".into(),
                token_hash: "h2".into(),
                paired_at: 1,
                revoked_at: None,
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
    fn executing_recovers_ambiguous_and_ledger_never_reuses_capacity() {
        let (r, _app) = test_runtime("crash");
        r.with_doc(|d| {
            d.devices.push(Device {
                id: "D".into(),
                name: "device".into(),
                token_hash: "a".repeat(64),
                paired_at: 1,
                revoked_at: None,
            });
            let request = request("I", "a");
            d.commands.push(JournalEntry {
                handle: sha(b"D\0I"),
                device_id: "D".into(),
                request_hash: sha(&serde_json::to_vec(&request).unwrap()),
                request,
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
                request: request(&format!("I{i}"), "a"),
                state: "rejected".into(),
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
        let cert = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&identity.cert_der)
            .unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(cert)).unwrap();
        let client = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let server = Arc::new(identity.tls().unwrap());
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
}
