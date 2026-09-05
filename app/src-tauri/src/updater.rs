//! Update channels: a closed `stable | nightly` choice mapped to exactly one
//! compiled endpoint. Tauri owns semver comparison, download, minisign
//! verification and install; build identity is version + bounded commit.
//!
//! # Contract
//! Updates have a closed `stable | nightly` setting; missing, unknown or damaged
//! values normalize to Stable. The webview owns no updater capability or URL.
//! `updater.rs` maps the enum to exactly one compiled HTTPS endpoint and uses
//! `UpdaterExt::updater_builder().endpoints(vec![endpoint])`; a Nightly failure
//! never falls back. Tauri 2.10.1 still owns semver comparison, archive download,
//! minisign verification and install. Build identity is only numeric version +
//! a bounded hex commit from `build.rs`.

use crate::error::DeckError;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

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
        _ => Err("update channel must be stable or nightly".into()),
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
        .map_err(|_| DeckError::from("configured update endpoint is invalid"))?;
    let mut builder = app
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|_| DeckError::from("configured update endpoint was rejected"))?;
    if let Some(pubkey) = pubkey {
        builder = builder.pubkey(pubkey);
    }
    let updater = builder
        .build()
        .map_err(|_| DeckError::from("updater could not be initialized"))?;
    updater
        .check()
        .await
        .map_err(|_| DeckError::from("update check failed"))
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
        return Err("expected update version is invalid".into());
    }
    let update = update_for_channel(&app, &channel)
        .await?
        .ok_or_else(|| "the selected update is no longer available".to_string())?;
    if update.version != expected_version {
        return Err("the selected update changed; check again".into());
    }
    let progress_app = app.clone();
    let finish_app = app.clone();
    crate::tmux_lifecycle::begin_app_update_install()?;
    let result = update
        .download_and_install(
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
        .await;
    if result.is_err() {
        crate::tmux_lifecycle::cancel_app_update_install();
    }
    result.map_err(|_| {
        DeckError::from("update download, signature verification, or installation failed")
    })
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
}
