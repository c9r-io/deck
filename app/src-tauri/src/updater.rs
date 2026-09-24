//! Update channels: a closed `stable | nightly` choice mapped to exactly one
//! compiled endpoint. Tauri owns semver comparison, download, minisign
//! verification and install; build identity is version + bounded commit.
//!
//! # Contract
//! Updates have a closed `stable | nightly` setting; missing, unknown or damaged
//! values normalize to Stable. The webview owns no updater capability or URL.
//! `updater.rs` maps the enum to exactly one compiled HTTPS endpoint and uses
//! `UpdaterExt::updater_builder().endpoints(vec![endpoint])`; a Nightly failure
//! never falls back. tauri-plugin-updater 2.10.1 owns semver comparison,
//! archive download and minisign verification (`Update::download`). Build
//! identity is only numeric version + a bounded hex commit from `build.rs`.
//!
//! deck installs the verified archive ITSELF (`install_bundle`) and never
//! calls the plugin's `install`/`download_and_install`: the plugin's macOS
//! installer (`updater.rs:1274-1305` in 2.10.1) runs `do shell script …
//! with administrator privileges` through OSAKit when renaming the bundle
//! is refused, and spawns a PATH-resolved `touch` after every successful
//! install — an admin password prompt and a shell child from deck, in
//! exactly the corporate standard-user layout whose EDR flagged deck
//! (`tests/edr_quiet.rs`). Before anything is downloaded or the lifecycle
//! flag is set, `writable_bundle` requires the installed `.app` and its
//! parent directory to be writable by this user (`access(W_OK)`, no spawn)
//! and otherwise returns `Perm` with a fixed message the sidebar shows;
//! the DMG is the way to update such an install. Staging and backup
//! directories are created NEXT TO the bundle so every rename stays on one
//! volume and the guard covers exactly what is touched; a failed swap puts
//! the previous bundle back. The plugin version is pinned by
//! `tests/edr_quiet.rs` so a bump re-reads its installer.

use crate::error::{DeckError, ErrorKind};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

/// Shown in the sidebar when an install is refused; `app.js` matches it.
const NOT_WRITABLE: &str =
    "the app bundle is not writable by this user; reinstall the DMG manually";

const STABLE_UPDATE_ENDPOINT: &str =
    "https://github.com/c9r-io/deck/releases/latest/download/latest.json";
const NIGHTLY_UPDATE_ENDPOINT: &str =
    "https://github.com/c9r-io/deck/releases/download/nightly-feed/latest.json";
const NIGHTLY_UPDATE_PUBKEY: &str = include_str!("../updater/nightly.pub.b64");

fn update_source(channel: &str) -> Result<(&'static str, Option<&'static str>), DeckError> {
    match channel {
        // Stable keeps using the key from tauri.conf.json. Nightly overrides
        // it with a separately generated key so compromise of the candidate
        // pipeline cannot mint an update accepted by Stable clients.
        "stable" => Ok((STABLE_UPDATE_ENDPOINT, None)),
        "nightly" => Ok((NIGHTLY_UPDATE_ENDPOINT, Some(NIGHTLY_UPDATE_PUBKEY.trim()))),
        _ => Err(DeckError::new(
            ErrorKind::Other,
            "update channel must be stable or nightly",
        )),
    }
}

fn strict_release_version(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (*part == "0" || !part.starts_with('0'))
        })
}

async fn update_for_channel(
    app: &AppHandle,
    channel: &str,
) -> Result<Option<tauri_plugin_updater::Update>, DeckError> {
    let (endpoint, pubkey) = update_source(channel)?;
    let endpoint = endpoint
        .parse()
        .map_err(|_| DeckError::new(ErrorKind::Other, "configured update endpoint is invalid"))?;
    let mut builder = app
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|_| DeckError::new(ErrorKind::Other, "configured update endpoint was rejected"))?;
    if let Some(pubkey) = pubkey {
        builder = builder.pubkey(pubkey);
    }
    let updater = builder
        .build()
        .map_err(|_| DeckError::new(ErrorKind::Other, "updater could not be initialized"))?;
    updater
        .check()
        .await
        .map_err(|_| DeckError::new(ErrorKind::Other, "update check failed"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateInfo {
    version: String,
    current_version: String,
    channel: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProgress {
    event: &'static str,
    chunk_length: usize,
    content_length: Option<u64>,
}

/// Select exactly one backend-owned endpoint. The webview can choose only the
/// closed channel enum and never supplies a URL or a fallback endpoint.
#[tauri::command]
pub(crate) async fn check_for_update(
    app: AppHandle,
    channel: String,
) -> Result<Option<UpdateInfo>, DeckError> {
    Ok(update_for_channel(&app, &channel)
        .await?
        .map(|update| UpdateInfo {
            version: update.version,
            current_version: update.current_version,
            channel,
        }))
}

/// Re-check the same single endpoint immediately before download so a stale
/// UI handle cannot install a different release. Tauri performs download,
/// minisign verification and installation; this command only reports progress.
#[tauri::command]
pub(crate) async fn install_update(
    app: AppHandle,
    channel: String,
    expected_version: String,
) -> Result<(), DeckError> {
    if !strict_release_version(&expected_version) {
        return Err(DeckError::new(
            ErrorKind::InvalidDoc,
            "expected update version is invalid",
        ));
    }
    let update = update_for_channel(&app, &channel).await?.ok_or_else(|| {
        DeckError::new(
            ErrorKind::Other,
            "the selected update is no longer available",
        )
    })?;
    if update.version != expected_version {
        return Err(DeckError::new(
            ErrorKind::Other,
            "the selected update changed; check again",
        ));
    }
    // Refuse, rather than let a permission error reach any privileged path,
    // before a byte is downloaded or the lifecycle flag is raised.
    let bundle = writable_bundle()?;
    let progress_app = app.clone();
    let finish_app = app.clone();
    crate::tmux_lifecycle::begin_app_update_install()?;
    let result = update
        .download(
            move |chunk_length, content_length| {
                let _ = progress_app.emit(
                    "update-download-progress",
                    UpdateProgress {
                        event: "progress",
                        chunk_length,
                        content_length,
                    },
                );
            },
            move || {
                let _ = finish_app.emit(
                    "update-download-progress",
                    UpdateProgress {
                        event: "finished",
                        chunk_length: 0,
                        content_length: None,
                    },
                );
            },
        )
        .await
        .map_err(|_| {
            DeckError::new(
                ErrorKind::Other,
                "update download or signature verification failed",
            )
        })
        .and_then(|archive| install_bundle(&bundle, &archive));
    if result.is_err() {
        crate::tmux_lifecycle::cancel_app_update_install();
    }
    result
}

/// The installed bundle this process runs from; `None` for a bare binary
/// outside a `*.app/Contents/MacOS/` layout, where there is nothing to
/// replace and an install is refused rather than guessed.
fn installed_bundle() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    crate::relaunch::app_bundle_for_executable(&executable)
}

/// `access(2)` with `W_OK`: the kernel's answer for THIS user, including
/// ACLs, without creating anything and without a process.
#[cfg(target_os = "macos")]
fn user_writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `path` is a valid NUL-terminated string for the whole call.
    unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(not(target_os = "macos"))]
fn user_writable(_path: &Path) -> bool {
    false
}

/// The bundle an update may replace: installed, and writable — together
/// with its parent directory, which the swap renames into — by this user.
fn writable_bundle() -> Result<PathBuf, DeckError> {
    let bundle = installed_bundle().ok_or_else(|| {
        DeckError::new(
            ErrorKind::Other,
            "updates install only into an installed app bundle",
        )
    })?;
    let parent = bundle
        .parent()
        .ok_or_else(|| DeckError::new(ErrorKind::Other, "app bundle has no parent directory"))?;
    if !user_writable(&bundle) || !user_writable(parent) {
        return Err(DeckError::new(ErrorKind::Perm, NOT_WRITABLE));
    }
    Ok(bundle)
}

/// Replace `bundle` with the `.app` of the same name inside `archive` (a
/// `.app.tar.gz` from the release feed whose signature the download already
/// verified). Staging and backup live next to the bundle; both are removed
/// afterwards, the backup only once the new bundle is in place. No process
/// is spawned.
pub(crate) fn install_bundle(bundle: &Path, archive: &[u8]) -> Result<(), DeckError> {
    let parent = bundle
        .parent()
        .ok_or_else(|| DeckError::new(ErrorKind::Other, "app bundle has no parent directory"))?;
    let name = bundle
        .file_name()
        .ok_or_else(|| DeckError::new(ErrorKind::Other, "app bundle has no name"))?;
    let stamp = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let staging = parent.join(format!(".deck-update-{stamp}"));
    let backup = parent.join(format!(".deck-previous-{stamp}"));
    std::fs::create_dir(&staging)?;
    let result = stage_and_swap(bundle, &staging.join(name), &staging, &backup, archive);
    let _ = std::fs::remove_dir_all(&staging);
    if result.is_ok() {
        let _ = std::fs::remove_dir_all(&backup);
    }
    result
}

fn stage_and_swap(
    bundle: &Path,
    fresh: &Path,
    staging: &Path,
    backup: &Path,
    archive: &[u8],
) -> Result<(), DeckError> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    // `unpack` refuses entries that would escape `staging`
    tar.unpack(staging)?;
    if !fresh.join("Contents").is_dir() {
        return Err(DeckError::new(
            ErrorKind::InvalidDoc,
            "update archive does not contain the app bundle",
        ));
    }
    std::fs::rename(bundle, backup)?;
    if let Err(error) = std::fs::rename(fresh, bundle) {
        // put the previous bundle back before reporting
        let _ = std::fs::rename(backup, bundle);
        return Err(error.into());
    }
    // what the plugin's `touch` did: a fresh modification time so
    // LaunchServices notices the replaced bundle
    if let Ok(dir) = std::fs::File::open(bundle) {
        let _ = dir.set_modified(SystemTime::now());
    }
    Ok(())
}

#[derive(Serialize)]
pub(crate) struct BuildIdentity {
    version: &'static str,
    commit: String,
}

#[tauri::command]
pub(crate) fn build_identity() -> BuildIdentity {
    let raw = env!("DECK_BUILD_COMMIT");
    let commit = if raw == "dev" {
        raw.to_string()
    } else {
        raw.chars().take(12).collect()
    };
    BuildIdentity {
        version: env!("CARGO_PKG_VERSION"),
        commit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_channel_endpoints_are_a_closed_single_choice() {
        assert_eq!(
            update_source("stable").unwrap(),
            (STABLE_UPDATE_ENDPOINT, None)
        );
        let nightly = update_source("nightly").unwrap();
        assert_eq!(nightly.0, NIGHTLY_UPDATE_ENDPOINT);
        assert_eq!(nightly.1, Some(NIGHTLY_UPDATE_PUBKEY.trim()));
        assert_ne!(
            nightly.1,
            Some("dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEM1MUFGNDJGODA1MEJENTMKUldSVHZWQ0FML1FheGJ0MDJEQ2ZKNFUxUnNpTlFjYlJmMmdpNlBDUUV4U29jd1ZXcmR0d3RPTGQK")
        );
        assert_ne!(STABLE_UPDATE_ENDPOINT, NIGHTLY_UPDATE_ENDPOINT);
        for invalid in ["", "beta", "night", "https://example.com/latest.json"] {
            assert!(update_source(invalid).is_err());
        }
    }

    #[test]
    fn update_versions_and_build_identity_are_public_bounded_values() {
        for valid in ["0.4.37", "1.0.0", "12.345.6789"] {
            assert!(strict_release_version(valid));
        }
        for invalid in [
            "0.4",
            "01.2.3",
            "0.4.37-nightly.1",
            "0.4.37+sha",
            "1.2.3.4",
            "1..3",
        ] {
            assert!(!strict_release_version(invalid));
        }
        let identity = build_identity();
        assert_eq!(identity.version, env!("CARGO_PKG_VERSION"));
        assert!(strict_release_version(identity.version));
        assert!(
            identity.commit == "dev"
                || ((7..=12).contains(&identity.commit.len())
                    && identity.commit.bytes().all(|b| b.is_ascii_hexdigit()))
        );
    }
    fn archive_with(top: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast());
            let mut tar = tar::Builder::new(gz);
            for (path, bytes) in files {
                let mut header = tar::Header::new_gnu();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                tar.append_data(&mut header, format!("{top}/{path}"), *bytes)
                    .unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
        }
        out
    }

    fn leftovers(parent: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(parent)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".deck-"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn install_swaps_the_bundle_next_to_itself_and_cleans_up() {
        let parent = std::env::temp_dir().join(format!("deck-updater-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::create_dir_all(parent.join("deck.app/Contents/MacOS")).unwrap();
        std::fs::write(parent.join("deck.app/Contents/MacOS/deck-app"), b"old").unwrap();
        let bundle = parent.join("deck.app");
        let archive = archive_with(
            "deck.app",
            &[
                ("Contents/Info.plist", b"plist"),
                ("Contents/MacOS/deck-app", b"new"),
            ],
        );
        install_bundle(&bundle, &archive).unwrap();
        assert_eq!(
            std::fs::read(bundle.join("Contents/MacOS/deck-app")).unwrap(),
            b"new"
        );
        assert!(bundle.join("Contents/Info.plist").is_file());
        assert!(leftovers(&parent).is_empty(), "{:?}", leftovers(&parent));
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn a_foreign_or_broken_archive_leaves_the_installed_bundle_untouched() {
        let parent = std::env::temp_dir().join(format!("deck-updater-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::create_dir_all(parent.join("deck.app/Contents/MacOS")).unwrap();
        std::fs::write(parent.join("deck.app/Contents/MacOS/deck-app"), b"old").unwrap();
        let bundle = parent.join("deck.app");
        let other = archive_with("other.app", &[("Contents/MacOS/x", b"x")]);
        assert!(install_bundle(&bundle, &other).is_err());
        assert!(install_bundle(&bundle, b"not a gzip stream").is_err());
        assert_eq!(
            std::fs::read(bundle.join("Contents/MacOS/deck-app")).unwrap(),
            b"old"
        );
        assert!(leftovers(&parent).is_empty(), "{:?}", leftovers(&parent));
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn install_refuses_shapeless_paths_and_a_missing_bundle_without_leftovers() {
        assert_eq!(
            install_bundle(Path::new("/"), b"").unwrap_err().message(),
            "app bundle has no parent directory"
        );
        let parent =
            std::env::temp_dir().join(format!("deck-updater-shape-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::create_dir_all(&parent).unwrap();
        assert_eq!(
            install_bundle(&parent.join(".."), b"")
                .unwrap_err()
                .message(),
            "app bundle has no name"
        );
        assert!(
            install_bundle(Path::new("/System/deck.app"), b"").is_err(),
            "staging next to an unwritable bundle fails before any unpack"
        );
        assert!(leftovers(Path::new("/System")).is_empty());
        // A valid archive for a bundle that is not there: nothing to swap.
        let archive = archive_with("deck.app", &[("Contents/MacOS/deck-app", b"new")]);
        let error = install_bundle(&parent.join("deck.app"), &archive).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Missing, "{error}");
        assert!(!parent.join("deck.app").exists());
        assert!(leftovers(&parent).is_empty(), "{:?}", leftovers(&parent));
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn progress_and_update_wire_shapes_are_camel_case() {
        let progress = serde_json::to_value(UpdateProgress {
            event: "progress",
            chunk_length: 512,
            content_length: Some(4096),
        })
        .unwrap();
        assert_eq!(
            progress,
            serde_json::json!({"event":"progress","chunkLength":512,"contentLength":4096})
        );
        let info = serde_json::to_value(UpdateInfo {
            version: "0.7.9".into(),
            current_version: "0.7.8".into(),
            channel: "nightly".into(),
        })
        .unwrap();
        assert_eq!(
            info,
            serde_json::json!({"version":"0.7.9","currentVersion":"0.7.8","channel":"nightly"})
        );
        let identity = serde_json::to_value(build_identity()).unwrap();
        assert_eq!(identity["version"], env!("CARGO_PKG_VERSION"));
        assert!(identity["commit"].is_string());
        assert_eq!(
            NOT_WRITABLE,
            "the app bundle is not writable by this user; reinstall the DMG manually"
        );
    }

    #[test]
    fn the_writability_guard_asks_the_kernel_for_this_user() {
        assert!(user_writable(&std::env::temp_dir()));
        assert!(!user_writable(Path::new("/System")));
        assert!(!user_writable(Path::new("/nonexistent/deck.app")));
        assert!(
            installed_bundle().is_none(),
            "a test binary is not an installed .app"
        );
        assert_eq!(
            writable_bundle().unwrap_err().kind(),
            ErrorKind::Other,
            "no bundle: refused before any writability question"
        );
    }
}
