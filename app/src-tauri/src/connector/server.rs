//! HTTPS listener for the closed v1 routes. Every unauthenticated byte is
//! bounded: at most `MAX_CONNECTIONS` connections and `MAX_PER_SOURCE` per
//! source address, a 5s TLS handshake, a 10s header read, a 30s connection
//! and a 4 KiB pairing body. A command body is read only after its Bearer
//! token authorizes.
//!
//! Network: the accept loop wakes at least every 500ms; every
//! `NETWORK_RECHECK` it re-reads the interfaces (`getifaddrs`, no spawn) and
//! stops the listener when the saved address is gone or is now carried by a
//! different interface or netmask than when it started. The UI then shows
//! "enabled, not listening" (`connector-changed` is emitted). A connection
//! whose local address is not the configured one is dropped unread.

use super::{
    buffer, output, snapshot, CommandRequest, Config, Identity, Runtime, CLIENT_UPGRADE_REQUIRED,
    COMMAND_EXPIRED,
};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;

const MAX_BODY: usize = 256 * 1024;
const MAX_RESPONSE: usize = 4 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONNECTIONS: usize = 16;
/// One address cannot hold every slot: a phone needs at most a couple of
/// concurrent requests, and slow unauthenticated connections from one peer
/// must not lock out another.
const MAX_PER_SOURCE: usize = 4;
const MAX_PAIR_BODY: usize = 4 * 1024;
const MAX_NATIVE_JOBS: usize = 8;
/// How often the running listener re-checks that its address, interface and
/// netmask are unchanged.
const NETWORK_RECHECK: Duration = Duration::from_secs(5);

/// Per-source connection accounting; a slot is released when its guard drops.
#[derive(Clone, Default)]
struct SourceSlots(Arc<std::sync::Mutex<std::collections::HashMap<IpAddr, usize>>>);

struct SourceSlot {
    slots: SourceSlots,
    source: IpAddr,
}

impl SourceSlots {
    fn try_acquire(&self, source: IpAddr) -> Option<SourceSlot> {
        let mut held = self.0.lock_or_recover();
        let count = held.entry(source).or_insert(0);
        if *count >= MAX_PER_SOURCE {
            return None;
        }
        *count += 1;
        Some(SourceSlot {
            slots: self.clone(),
            source,
        })
    }
}

impl Drop for SourceSlot {
    fn drop(&mut self) {
        let mut held = self.slots.0.lock_or_recover();
        if let Some(count) = held.get_mut(&self.source) {
            *count -= 1;
            if *count == 0 {
                held.remove(&self.source);
            }
        }
    }
}

type Resp = Response<Full<Bytes>>;

fn response(status: StatusCode, value: Value) -> Resp {
    let mut bytes = serde_json::to_vec(&value)
        .unwrap_or_else(|_| b"{\"error\":{\"code\":\"internal\"}}".to_vec());
    let status = if bytes.len() > MAX_RESPONSE {
        bytes = b"{\"error\":{\"code\":\"response-too-large\"}}".to_vec();
        StatusCode::INSUFFICIENT_STORAGE
    } else {
        status
    };
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .body(Full::new(Bytes::from(bytes)))
        .unwrap()
}

fn error(status: StatusCode, code: &'static str) -> Resp {
    response(status, json!({"error":{"code":code}}))
}

fn mapped(error_value: &DeckError) -> Resp {
    if error_value.message() == "connector unavailable" {
        return error(StatusCode::SERVICE_UNAVAILABLE, "connector-disabled");
    }
    if error_value.message() == COMMAND_EXPIRED {
        // Not 404: the outcome is unknown, so the phone must not retry.
        return error(StatusCode::GONE, "expired");
    }
    if error_value.message() == CLIENT_UPGRADE_REQUIRED {
        // A phone build without command sequences cannot be admitted safely.
        return error(StatusCode::UPGRADE_REQUIRED, "upgrade-required");
    }
    if error_value.message() == "unsupported-target" {
        return error(StatusCode::BAD_REQUEST, "unsupported-target");
    }
    match error_value.kind() {
        ErrorKind::Missing => error(StatusCode::NOT_FOUND, "not-found"),
        ErrorKind::Perm => error(StatusCode::UNAUTHORIZED, "unauthorized"),
        ErrorKind::ContextChanged => error(StatusCode::CONFLICT, "context-changed"),
        ErrorKind::DiskFull => error(StatusCode::INSUFFICIENT_STORAGE, "capacity"),
        ErrorKind::Invalid | ErrorKind::InvalidDoc => error(StatusCode::BAD_REQUEST, "invalid"),
        _ => error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    }
}

pub(super) fn spawn(
    runtime: Arc<Runtime>,
    cfg: Config,
    identity: Identity,
    epoch: u64,
    network_unchanged: impl Fn() -> bool + Send + 'static,
) -> Result<u16, DeckError> {
    let ip: IpAddr = cfg
        .address
        .parse()
        .map_err(|_| DeckError::new(ErrorKind::Invalid, "invalid connector address"))?;
    let tls = Arc::new(identity.tls()?);
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("deck-connector".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_io()
                .enable_time()
                .build();
            if let Ok(rt) = rt {
                rt.block_on(run(
                    runtime,
                    SocketAddr::new(ip, cfg.port),
                    tls,
                    epoch,
                    ready_tx,
                    network_unchanged,
                ));
            } else {
                let _ = ready_tx.send(None);
            }
        })
        .map_err(|_| DeckError::new(ErrorKind::Other, "connector server could not start"))?;
    match ready_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(Some(port)) => Ok(port),
        _ => Err(DeckError::new(
            ErrorKind::Other,
            "connector address could not be bound",
        )),
    }
}

async fn run(
    runtime: Arc<Runtime>,
    addr: SocketAddr,
    tls: Arc<rustls::ServerConfig>,
    epoch: u64,
    ready: std::sync::mpsc::SyncSender<Option<u16>>,
    network_unchanged: impl Fn() -> bool,
) {
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(value) => value,
        Err(_) => {
            let _ = ready.send(None);
            return;
        }
    };
    let bound_port = listener.local_addr().map(|address| address.port()).ok();
    let Some(bound_port) = bound_port else {
        let _ = ready.send(None);
        return;
    };
    runtime.running_epoch.store(epoch, Ordering::SeqCst);
    let _ = ready.send(Some(bound_port));
    let acceptor = tokio_rustls::TlsAcceptor::from(tls);
    let connections = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    let sources = SourceSlots::default();
    let jobs = Arc::new(tokio::sync::Semaphore::new(MAX_NATIVE_JOBS));
    let local = SocketAddr::new(addr.ip(), bound_port);
    let mut checked = Instant::now();
    let mut network_changed = false;
    while runtime.server_epoch.load(Ordering::SeqCst) == epoch {
        if checked.elapsed() >= NETWORK_RECHECK {
            checked = Instant::now();
            if !network_unchanged() {
                network_changed = true;
                break;
            }
        }
        let accepted = tokio::time::timeout(Duration::from_millis(500), listener.accept()).await;
        let Ok(Ok((tcp, peer))) = accepted else {
            continue;
        };
        if tcp.local_addr().ok() != Some(local) {
            continue;
        }
        let Some(source_slot) = sources.try_acquire(peer.ip()) else {
            continue;
        };
        let Ok(connection_permit) = connections.clone().try_acquire_owned() else {
            continue;
        };
        let acceptor = acceptor.clone();
        let state = runtime.clone();
        let jobs = jobs.clone();
        tokio::spawn(async move {
            let _connection_permit = connection_permit;
            let _source_slot = source_slot;
            let Ok(Ok(tls_stream)) =
                tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await
            else {
                return;
            };
            let service = hyper::service::service_fn(move |request| {
                let state = state.clone();
                let jobs = jobs.clone();
                async move { Ok::<_, Infallible>(handle(state, jobs, epoch, request).await) }
            });
            let io = TokioIo::new(tls_stream);
            let mut builder = hyper::server::conn::http1::Builder::new();
            builder.keep_alive(false);
            builder.max_headers(32).max_buf_size(32 * 1024);
            builder
                .timer(TokioTimer::new())
                .header_read_timeout(Duration::from_secs(10));
            let _ = tokio::time::timeout(CONNECTION_TIMEOUT, builder.serve_connection(io, service))
                .await;
        });
    }
    let _ = runtime
        .running_epoch
        .compare_exchange(epoch, 0, Ordering::SeqCst, Ordering::SeqCst);
    if network_changed {
        if let Some(app) = runtime.app.as_ref() {
            use tauri::Emitter;
            let _ = app.emit("connector-changed", ());
        }
    }
}

async fn body(request: Request<Incoming>, limit: usize) -> Result<Vec<u8>, Resp> {
    if request
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > limit)
    {
        return Err(error(StatusCode::PAYLOAD_TOO_LARGE, "body-too-large"));
    }
    Limited::new(request.into_body(), limit)
        .collect()
        .await
        .map(|collected| collected.to_bytes().to_vec())
        .map_err(|_| error(StatusCode::PAYLOAD_TOO_LARGE, "body-too-large"))
}

fn token(request: &Request<Incoming>) -> Option<String> {
    request
        .headers()
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .map(str::to_owned)
}

fn decode_segment(segment: &str) -> Option<String> {
    let mut out = Vec::new();
    let bytes = segment.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let decoded =
                u8::from_str_radix(std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?, 16)
                    .ok()?;
            if decoded == b'/' {
                return None;
            }
            out.push(decoded);
            index += 3;
        } else {
            if bytes[index] == b'/' {
                return None;
            }
            out.push(bytes[index]);
            index += 1;
        }
    }
    let value = String::from_utf8(out).ok()?;
    (!value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control))
        .then_some(value)
}

async fn native(
    state: Arc<Runtime>,
    jobs: Arc<tokio::sync::Semaphore>,
    epoch: u64,
    work: impl FnOnce(Arc<Runtime>) -> Resp + Send + 'static,
) -> Resp {
    if !state.epoch_active(epoch) {
        return error(StatusCode::SERVICE_UNAVAILABLE, "connector-disabled");
    }
    let Ok(job_permit) = jobs.try_acquire_owned() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "busy");
    };
    let checked = state.clone();
    let task = tokio::task::spawn_blocking(move || {
        let _job_permit = job_permit;
        let _deadline = crate::session_runtime::Deadline::until(Instant::now() + REQUEST_TIMEOUT);
        if !checked.epoch_active(epoch) {
            return error(StatusCode::SERVICE_UNAVAILABLE, "connector-disabled");
        }
        let response = work(checked.clone());
        if checked.epoch_active(epoch) {
            response
        } else {
            error(StatusCode::SERVICE_UNAVAILABLE, "connector-disabled")
        }
    });
    match tokio::time::timeout(REQUEST_TIMEOUT, task).await {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
        Err(_) => error(StatusCode::REQUEST_TIMEOUT, "timeout"),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PairBody {
    code: String,
    device_name: String,
}

async fn handle(
    state: Arc<Runtime>,
    jobs: Arc<tokio::sync::Semaphore>,
    epoch: u64,
    request: Request<Incoming>,
) -> Resp {
    if request.uri().query().is_some() {
        return error(StatusCode::BAD_REQUEST, "invalid");
    }
    if !state.epoch_active(epoch) {
        return error(StatusCode::SERVICE_UNAVAILABLE, "connector-disabled");
    }
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let bearer = token(&request);

    if method == Method::POST && path == "/v1/pair" {
        let bytes = match body(request, MAX_PAIR_BODY).await {
            Ok(value) => value,
            Err(response) => return response,
        };
        let pair: PairBody = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => return error(StatusCode::BAD_REQUEST, "invalid"),
        };
        return native(state, jobs, epoch, move |state| {
            state
                .pair(epoch, &pair.code, &pair.device_name)
                .map(|value| response(StatusCode::OK, value))
                .unwrap_or_else(|failure| mapped(&failure))
        })
        .await;
    }

    if method == Method::POST && path == "/v1/commands" {
        // Authenticate from the headers before reading (or waiting for) any
        // body; the native step authorizes again under the current epoch.
        let authorized = bearer
            .as_deref()
            .ok_or_else(|| DeckError::new(ErrorKind::Perm, "unauthorized"))
            .and_then(|token| state.authorize(epoch, token));
        if let Err(failure) = authorized {
            return mapped(&failure);
        }
        let bytes = match body(request, MAX_BODY).await {
            Ok(value) => value,
            Err(response) => return response,
        };
        let command: CommandRequest = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => return error(StatusCode::BAD_REQUEST, "invalid"),
        };
        return native(state, jobs, epoch, move |state| {
            let device = match bearer
                .as_deref()
                .ok_or_else(|| DeckError::new(ErrorKind::Perm, "unauthorized"))
                .and_then(|token| state.authorize(epoch, token))
            {
                Ok(value) => value,
                Err(failure) => return mapped(&failure),
            };
            match state.accept(epoch, &device, command) {
                Ok(_) if crate::smoke_faults::take("connector-after-accept") => {
                    error(StatusCode::GATEWAY_TIMEOUT, "timeout")
                }
                Ok(value) => response(StatusCode::ACCEPTED, serde_json::to_value(value).unwrap()),
                Err(failure) => mapped(&failure),
            }
        })
        .await;
    }

    if method != Method::GET {
        return error(StatusCode::NOT_FOUND, "not-found");
    }
    native(state, jobs, epoch, move |state| {
        let device = match bearer
            .as_deref()
            .ok_or_else(|| DeckError::new(ErrorKind::Perm, "unauthorized"))
            .and_then(|token| state.authorize(epoch, token))
        {
            Ok(value) => value,
            Err(failure) => return mapped(&failure),
        };
        if path == "/v1/snapshot" {
            return state
                .app
                .as_ref()
                .ok_or_else(|| DeckError::new(ErrorKind::Other, "connector unavailable"))
                .and_then(snapshot)
                .map(|value| response(StatusCode::OK, value))
                .unwrap_or_else(|failure| mapped(&failure));
        }
        if let Some(id) = path.strip_prefix("/v1/commands/").and_then(decode_segment) {
            return state
                .command_result(&device, &id)
                .map(|value| response(StatusCode::OK, serde_json::to_value(value).unwrap()))
                .unwrap_or_else(|failure| mapped(&failure));
        }
        if let Some(rest) = path.strip_prefix("/v1/cards/") {
            if let Some((raw, leaf)) = rest.rsplit_once('/') {
                if let Some(id) = decode_segment(raw) {
                    return match leaf {
                        "output" => output(&id)
                            .map(|value| response(StatusCode::OK, value))
                            .unwrap_or_else(|failure| mapped(&failure)),
                        "buffer" => state
                            .app
                            .as_ref()
                            .ok_or_else(|| {
                                DeckError::new(ErrorKind::Other, "connector unavailable")
                            })
                            .and_then(|app| buffer(app, &id))
                            .map(|value| response(StatusCode::OK, value))
                            .unwrap_or_else(|failure| mapped(&failure)),
                        _ => error(StatusCode::NOT_FOUND, "not-found"),
                    };
                }
            }
        }
        error(StatusCode::NOT_FOUND, "not-found")
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use rustls::pki_types::{CertificateDer, ServerName};
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::atomic::AtomicU64;
    use std::sync::Mutex;

    #[test]
    fn wire_limits_match_v1_contract() {
        assert_eq!(MAX_BODY, 256 * 1024);
        assert_eq!(MAX_RESPONSE, 4 * 1024 * 1024);
        assert!(
            response(StatusCode::OK, json!({"text":"x".repeat(MAX_RESPONSE)}))
                .status()
                .is_server_error()
        );
        assert!(decode_segment("a%2Fb").is_none());
        assert_eq!(decode_segment("safe-1").as_deref(), Some("safe-1"));
        let disabled = mapped(&DeckError::new(ErrorKind::Other, "connector unavailable"));
        assert_eq!(disabled.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_ne!(disabled.status(), StatusCode::UNAUTHORIZED);
        let expired = mapped(&DeckError::new(ErrorKind::ContextChanged, COMMAND_EXPIRED));
        assert_eq!(expired.status(), StatusCode::GONE);
    }

    #[test]
    fn listener_stops_when_its_network_changes() {
        let path = std::env::temp_dir().join(format!(
            "deck-connector-network-test-{}-{}",
            std::process::id(),
            super::super::now()
        ));
        let mut doc = super::super::DiskDoc::fresh().unwrap();
        doc.config.enabled = true;
        let runtime = Arc::new(Runtime {
            app: None,
            path: path.clone(),
            doc: Mutex::new(Ok(doc)),
            pairing: Mutex::new(None),
            lifecycle: Mutex::new(()),
            server_epoch: AtomicU64::new(1),
            running_epoch: AtomicU64::new(0),
        });
        let config = Config {
            enabled: true,
            address: "127.0.0.1".into(),
            port: 0,
            interface: None,
        };
        let identity = Identity::generate("127.0.0.1").unwrap();
        spawn(runtime.clone(), config, identity, 1, || false).unwrap();
        assert_eq!(runtime.running_epoch.load(Ordering::SeqCst), 1);
        let deadline = Instant::now() + NETWORK_RECHECK + Duration::from_secs(3);
        while runtime.running_epoch.load(Ordering::SeqCst) != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(
            runtime.running_epoch.load(Ordering::SeqCst),
            0,
            "a changed network stops the listener"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn unauthenticated_resources_are_bounded_per_source_and_in_time() {
        assert_eq!(HANDSHAKE_TIMEOUT, Duration::from_secs(5));
        const { assert!(MAX_PER_SOURCE >= 2 && MAX_PER_SOURCE <= 4) };
        const { assert!(MAX_PER_SOURCE < MAX_CONNECTIONS) };
        assert_eq!(MAX_PAIR_BODY, 4 * 1024);

        let slots = SourceSlots::default();
        let one: IpAddr = "192.0.2.1".parse().unwrap();
        let two: IpAddr = "192.0.2.2".parse().unwrap();
        let held = (0..MAX_PER_SOURCE)
            .map(|_| slots.try_acquire(one).unwrap())
            .collect::<Vec<_>>();
        assert!(slots.try_acquire(one).is_none(), "one source is capped");
        let other = slots.try_acquire(two);
        assert!(other.is_some(), "another source still gets a slot");
        drop(held);
        assert!(
            slots.try_acquire(one).is_some(),
            "dropped slots are released"
        );
        drop(other);
        assert!(slots.0.lock().unwrap().get(&two).is_none());
    }

    #[test]
    fn inactive_epoch_and_exhausted_native_jobs_fail_closed_without_unauthorized() {
        let path =
            std::env::temp_dir().join(format!("deck-connector-server-test-{}", std::process::id()));
        let mut doc = super::super::DiskDoc::fresh().unwrap();
        doc.config = Config {
            enabled: true,
            address: "127.0.0.1".into(),
            port: 8443,
            interface: None,
        };
        super::super::save(&path, &doc).unwrap();
        let runtime = Arc::new(Runtime {
            app: None,
            path,
            doc: Mutex::new(Ok(doc)),
            pairing: Mutex::new(None),
            lifecycle: Mutex::new(()),
            server_epoch: AtomicU64::new(1),
            running_epoch: AtomicU64::new(1),
        });
        let async_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let busy = async_runtime.block_on(native(
            runtime.clone(),
            Arc::new(tokio::sync::Semaphore::new(0)),
            1,
            |_| response(StatusCode::OK, json!({})),
        ));
        assert_eq!(busy.status(), StatusCode::SERVICE_UNAVAILABLE);
        runtime.server_epoch.store(2, Ordering::SeqCst);
        let inactive = async_runtime.block_on(native(
            runtime,
            Arc::new(tokio::sync::Semaphore::new(1)),
            1,
            |_| response(StatusCode::OK, json!({})),
        ));
        assert_eq!(inactive.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_ne!(inactive.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn ephemeral_loopback_server_uses_real_tls_and_pair_route() {
        let path = std::env::temp_dir().join(format!(
            "deck-connector-loopback-test-{}-{}",
            std::process::id(),
            super::super::now()
        ));
        let mut doc = super::super::DiskDoc::fresh().unwrap();
        doc.config = Config {
            enabled: true,
            address: "127.0.0.1".into(),
            port: 0,
            interface: None,
        };
        super::super::save(&path, &doc).unwrap();
        let runtime = Arc::new(Runtime {
            app: None,
            path: path.clone(),
            doc: Mutex::new(Ok(doc)),
            pairing: Mutex::new(Some(super::super::Pairing {
                code: "smoke-code".into(),
                expires_at: super::super::now() + 30,
            })),
            lifecycle: Mutex::new(()),
            server_epoch: AtomicU64::new(1),
            running_epoch: AtomicU64::new(0),
        });
        let identity = Identity::generate("127.0.0.1").unwrap();
        let cert = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&identity.cert_der)
            .unwrap();
        let port = spawn(
            runtime.clone(),
            Config {
                enabled: true,
                address: "127.0.0.1".into(),
                port: 0,
                interface: None,
            },
            identity,
            1,
            || true,
        )
        .unwrap();
        assert_ne!(port, 0);

        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(cert)).unwrap();
        let client = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let name = ServerName::IpAddress("127.0.0.1".parse::<IpAddr>().unwrap().into());
        let conn = rustls::ClientConnection::new(client.clone(), name).unwrap();
        let mut stream = rustls::StreamOwned::new(conn, tcp);
        let body = br#"{"code":"smoke-code","deviceName":"fixture"}"#;
        write!(
            stream,
            "POST /v1/pair HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
        stream.flush().unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"deviceId\""));
        assert!(response.contains("\"token\""));

        // An unauthenticated command must be refused from its headers alone:
        // the server never waits for (or buffers) a body it has not
        // authenticated. The declared body is never sent.
        let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let name = ServerName::IpAddress("127.0.0.1".parse::<IpAddr>().unwrap().into());
        let conn = rustls::ClientConnection::new(client.clone(), name).unwrap();
        let mut stream = rustls::StreamOwned::new(conn, tcp);
        write!(
            stream,
            "POST /v1/commands HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 200000\r\n\r\n"
        )
        .unwrap();
        stream.flush().unwrap();
        let mut head = [0u8; 12];
        stream.read_exact(&mut head).unwrap();
        assert_eq!(&head, b"HTTP/1.1 401");
        runtime.server_epoch.store(2, Ordering::SeqCst);
        let _ = std::fs::remove_file(path);
    }

    /// One HTTPS request over loopback; returns (status, body).
    fn exchange(
        client: &Arc<rustls::ClientConfig>,
        port: u16,
        head: &str,
        body: &[u8],
    ) -> (u16, String) {
        let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let name = ServerName::IpAddress("127.0.0.1".parse::<IpAddr>().unwrap().into());
        let conn = rustls::ClientConnection::new(client.clone(), name).unwrap();
        let mut stream = rustls::StreamOwned::new(conn, tcp);
        write!(
            stream,
            "{head}Host: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
        stream.flush().unwrap();
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        let status = response[9..12].parse().unwrap();
        let body = response.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
        (status, body)
    }

    #[test]
    fn f4_http_post_admission_refuses_a_retired_command_without_a_prior_get() {
        let path = std::env::temp_dir().join(format!(
            "deck-connector-f4-http-{}-{}",
            std::process::id(),
            super::super::now()
        ));
        let mut doc = super::super::DiskDoc::fresh().unwrap();
        doc.config = Config {
            enabled: true,
            address: "127.0.0.1".into(),
            port: 0,
            interface: None,
        };
        super::super::save(&path, &doc).unwrap();
        let runtime = Arc::new(Runtime {
            app: None,
            path: path.clone(),
            doc: Mutex::new(Ok(doc)),
            pairing: Mutex::new(Some(super::super::Pairing {
                code: "f4-code".into(),
                expires_at: super::super::now() + 30,
            })),
            lifecycle: Mutex::new(()),
            server_epoch: AtomicU64::new(1),
            running_epoch: AtomicU64::new(0),
        });
        let identity = Identity::generate("127.0.0.1").unwrap();
        let cert = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&identity.cert_der)
            .unwrap();
        let config = Config {
            enabled: true,
            address: "127.0.0.1".into(),
            port: 0,
            interface: None,
        };
        let port = spawn(runtime.clone(), config, identity, 1, || true).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(cert)).unwrap();
        let client = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let (status, paired) = exchange(
            &client,
            port,
            "POST /v1/pair HTTP/1.1\r\n",
            br#"{"code":"f4-code","deviceName":"fixture"}"#,
        );
        assert_eq!(status, 200, "{paired}");
        let paired: Value = serde_json::from_str(&paired).unwrap();
        let token = paired["token"].as_str().unwrap().to_owned();
        let device = paired["deviceId"].as_str().unwrap().to_owned();
        let post = |body: &Value| {
            exchange(
                &client,
                port,
                &format!("POST /v1/commands HTTP/1.1\r\nAuthorization: Bearer {token}\r\n"),
                &serde_json::to_vec(body).unwrap(),
            )
        };
        let command = |id: &str, seq: Option<u64>| {
            let mut value = json!({"id":id,"kind":"buffer-add","cardId":"C1","expectedRevision":"1","payload":{"text":"phone note"}});
            if let Some(seq) = seq {
                value["seq"] = json!(seq);
            }
            value
        };
        let first = command("first", Some(1));
        let (status, body) = post(&first);
        assert_eq!(status, 202, "{body}");
        // Two identical POSTs in flight: one admission.
        let duplicate = command("dup", Some(2));
        let threads = (0..2)
            .map(|_| {
                let (client, token, duplicate) = (client.clone(), token.clone(), duplicate.clone());
                std::thread::spawn(move || {
                    exchange(
                        &client,
                        port,
                        &format!("POST /v1/commands HTTP/1.1\r\nAuthorization: Bearer {token}\r\n"),
                        &serde_json::to_vec(&duplicate).unwrap(),
                    )
                    .0
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            assert_eq!(thread.join().unwrap(), 202);
        }
        // Finish `first`, then retire it behind later history.
        let handle = super::super::sha(format!("{device}\0first").as_bytes());
        runtime
            .with_doc(|d| {
                let c = d.commands.iter_mut().find(|c| c.handle == handle).unwrap();
                c.state = "applied".into();
                c.result = Some(json!({"cardId":"C1","entryId":"E1","revision":"2"}));
                let template = c.clone();
                for index in 0..super::super::MAX_TOMBSTONES {
                    let id = format!("later-{index}");
                    let mut later = template.clone();
                    later.handle = super::super::sha(format!("{device}\0{id}").as_bytes());
                    later.id = id;
                    later.seq = Some(10 + index as u64);
                    d.commands.push(later);
                }
                Ok(())
            })
            .unwrap();
        let entries = runtime.read(|d| d.commands.len()).unwrap();
        runtime
            .read(|d| assert!(d.commands.iter().all(|c| c.id != "first")))
            .unwrap();
        // The exact retired command, POSTed directly (no GET first).
        let (status, body) = post(&first);
        assert_eq!(status, 410, "{body}");
        assert!(body.contains("expired"));
        assert_eq!(runtime.read(|d| d.commands.len()).unwrap(), entries);
        runtime
            .read(|d| {
                assert_eq!(
                    d.commands.iter().filter(|c| c.state == "accepted").count(),
                    1,
                    "only `dup` is pending"
                );
            })
            .unwrap();
        // An older phone build without sequences is told to upgrade.
        let (status, body) = post(&command("legacy", None));
        assert_eq!(status, 426, "{body}");
        // New work keeps flowing.
        let (status, body) = post(&command(
            "next",
            Some(super::super::MAX_TOMBSTONES as u64 + 100),
        ));
        assert_eq!(status, 202, "{body}");
        runtime.server_epoch.store(2, Ordering::SeqCst);
        let _ = std::fs::remove_file(path);
    }
}
