//! One certified en→zh-Hans data pack. All URLs, sizes and digests are fixed;
//! only an explicit Settings command downloads it. Assets are never executable.
use crate::datadir;
use crate::error::{DeckError, ErrorKind};
use flate2::read::GzDecoder;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) const PACK_ID: &str = "en-zh-hans-mozilla-base-memory-v1";
pub(crate) const LIVE_BYTES: usize = 4096;
pub(crate) const SELECTION_BYTES: usize = 16384;
pub(crate) const DOCUMENT_CHOICES: [usize; 2] = [8192, 16384];
pub(super) const DOWNLOAD_BYTES: u64 = 36_745_493;
pub(super) const INSTALLED_BYTES: u64 = 49_913_927;
const BASE: &str = "https://storage.googleapis.com/moz-fx-translations-data--303e-prod-translations-data/models/en-zh/llmaat_finetune10M_qe8_f2_ByQcSxGXQRqGi-UTxYE43g/exported/";
static STAGE_ID: AtomicU64 = AtomicU64::new(1);

struct Asset {
    name: &'static str,
    compressed_bytes: u64,
    bytes: u64,
    sha256: &'static str,
}
const ASSETS: [Asset; 4] = [
    Asset {
        name: "lex.50.50.enzh.s2t.bin",
        compressed_bytes: 2_536_039,
        bytes: 4_485_184,
        sha256: "8575d8daa10e2dbff316dcdf8e1ce475357bcc2c92bdc63b736a2d5add22f681",
    },
    Asset {
        name: "model.enzh.intgemm.alphas.bin",
        compressed_bytes: 33_375_922,
        bytes: 43_849_787,
        sha256: "4e5accc141373565ddc8fa1565bceaa8d0c3482a82cab8131c719ebcc6c2157c",
    },
    Asset {
        name: "srcvocab.enzh.spm",
        compressed_bytes: 407_784,
        bytes: 806_952,
        sha256: "bd9b65504acc6d9726dd281f7defc2adb7c2c22d0688fe2f84697de25197c8c5",
    },
    Asset {
        name: "trgvocab.enzh.spm",
        compressed_bytes: 425_748,
        bytes: 772_004,
        sha256: "aded6993c36e440284d11cec3f6b8aef9c0e43188a772d80be342a713adf223d",
    },
];

fn error(code: &'static str) -> DeckError {
    DeckError::new(ErrorKind::Other, code)
}
fn private_real_directory(path: &Path) -> Result<(), DeckError> {
    if path.exists() {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| error("translation-model-download-failed"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(error("translation-model-download-failed"));
        }
    } else {
        std::fs::create_dir(path).map_err(|_| error("translation-model-download-failed"))?;
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| error("translation-model-download-failed"))
}
pub(super) fn directory() -> PathBuf {
    datadir::deck_dir().join("models/translation").join(PACK_ID)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PackStatus {
    pub installed: bool,
    pub corrupt: bool,
    pub id: &'static str,
    pub version: &'static str,
    pub source_language: &'static str,
    pub target_language: &'static str,
    pub provider: &'static str,
    pub engine_compatibility: &'static str,
    pub download_bytes: u64,
    pub installed_bytes: u64,
    pub live_bytes: usize,
    pub selection_bytes: usize,
    pub document_choices: [usize; 2],
}

fn status_for(installed: bool, corrupt: bool) -> PackStatus {
    PackStatus {
        installed,
        corrupt,
        id: PACK_ID,
        version: "1",
        source_language: "en",
        target_language: "zh-Hans",
        provider: "bergamot",
        engine_compatibility: "9271618ebbdc5d21ac4dc4df9e72beb7ce644774",
        download_bytes: DOWNLOAD_BYTES,
        installed_bytes: INSTALLED_BYTES,
        live_bytes: LIVE_BYTES,
        selection_bytes: SELECTION_BYTES,
        document_choices: DOCUMENT_CHOICES,
    }
}

fn digest(path: &Path) -> Result<String, DeckError> {
    let mut file = std::fs::File::open(path).map_err(|_| error("translation-model-corrupt"))?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let size = file
            .read(&mut buffer)
            .map_err(|_| error("translation-model-corrupt"))?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(super) fn verify_at(dir: &Path) -> Result<(), DeckError> {
    if !dir.exists() {
        return Err(error("translation-model-missing"));
    }
    let metadata =
        std::fs::symlink_metadata(dir).map_err(|_| error("translation-model-corrupt"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(error("translation-model-corrupt"));
    }
    for asset in &ASSETS {
        let path = dir.join(asset.name);
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|_| error("translation-model-corrupt"))?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() != asset.bytes
            || metadata.permissions().mode() & 0o111 != 0
            || digest(&path)? != asset.sha256
        {
            return Err(error("translation-model-corrupt"));
        }
    }
    Ok(())
}
pub(super) fn verify() -> Result<(), DeckError> {
    verify_at(&directory())
}
pub(super) fn status() -> PackStatus {
    match verify() {
        Ok(()) => status_for(true, false),
        Err(_) if directory().exists() => status_for(false, true),
        Err(_) => status_for(false, false),
    }
}

async fn download_asset(
    client: &reqwest::Client,
    asset: &Asset,
    stage: &Path,
) -> Result<(), DeckError> {
    let url = format!("{BASE}{}.gz", asset.name);
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| error("translation-model-download-failed"))?;
    if !response.status().is_success() {
        return Err(error("translation-model-download-failed"));
    }
    let mut compressed = Vec::with_capacity(asset.compressed_bytes as usize);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| error("translation-model-download-failed"))?
    {
        if compressed.len() + chunk.len() > asset.compressed_bytes as usize {
            return Err(error("translation-model-download-failed"));
        }
        compressed.extend_from_slice(&chunk);
    }
    if compressed.len() as u64 != asset.compressed_bytes {
        return Err(error("translation-model-download-failed"));
    }
    let mut decoder = GzDecoder::new(compressed.as_slice()).take(asset.bytes + 1);
    let path = stage.join(asset.name);
    let mut file =
        datadir::open_private(&path).map_err(|_| error("translation-model-download-failed"))?;
    let written = std::io::copy(&mut decoder, &mut file)
        .map_err(|_| error("translation-model-download-failed"))?;
    file.flush()
        .map_err(|_| error("translation-model-download-failed"))?;
    if written != asset.bytes
        || digest(&path).map_err(|_| error("translation-model-download-failed"))? != asset.sha256
    {
        return Err(error("translation-model-download-failed"));
    }
    Ok(())
}

async fn install_at(final_dir: PathBuf) -> Result<PackStatus, DeckError> {
    if verify_at(&final_dir).is_ok() {
        return Ok(status_for(true, false));
    }
    if final_dir.exists() {
        return Err(error("translation-model-corrupt"));
    }
    let parent = final_dir
        .parent()
        .ok_or_else(|| error("translation-model-download-failed"))?;
    let models = parent
        .parent()
        .ok_or_else(|| error("translation-model-download-failed"))?;
    let deck_dir = models
        .parent()
        .ok_or_else(|| error("translation-model-download-failed"))?;
    private_real_directory(deck_dir)?;
    private_real_directory(models)?;
    private_real_directory(parent)?;
    let stage = parent.join(format!(
        ".{PACK_ID}-staging-{}-{}",
        std::process::id(),
        STAGE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    datadir::create_private_dir(&stage).map_err(|_| error("translation-model-download-failed"))?;
    let result = async {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|_| error("translation-model-download-failed"))?;
        for asset in &ASSETS {
            download_asset(&client, asset, &stage).await?;
        }
        verify_at(&stage).map_err(|_| error("translation-model-download-failed"))?;
        std::fs::rename(&stage, &final_dir)
            .map_err(|_| error("translation-model-download-failed"))?;
        Ok(status_for(true, false))
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&stage);
    }
    result
}
pub(super) async fn install() -> Result<PackStatus, DeckError> {
    install_at(directory()).await
}

pub(super) fn delete() -> Result<(), DeckError> {
    let dir = directory();
    if !dir.exists() {
        return Ok(());
    }
    let metadata =
        std::fs::symlink_metadata(&dir).map_err(|_| error("translation-model-delete-failed"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(error("translation-model-delete-failed"));
    }
    for entry in std::fs::read_dir(&dir).map_err(|_| error("translation-model-delete-failed"))? {
        let entry = entry.map_err(|_| error("translation-model-delete-failed"))?;
        if !ASSETS.iter().any(|asset| entry.file_name() == asset.name)
            || !entry
                .file_type()
                .map_err(|_| error("translation-model-delete-failed"))?
                .is_file()
        {
            return Err(error("translation-model-delete-failed"));
        }
    }
    for asset in &ASSETS {
        let path = dir.join(asset.name);
        if path.exists() {
            std::fs::remove_file(path).map_err(|_| error("translation-model-delete-failed"))?;
        }
    }
    std::fs::remove_dir(dir).map_err(|_| error("translation-model-delete-failed"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_wrong_size_and_symlink_never_verify() {
        let root = std::env::temp_dir().join(format!(
            "deck-pack-cert-{}-{}",
            std::process::id(),
            STAGE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        assert!(verify_at(&root).is_err());
        for asset in &ASSETS {
            std::fs::write(root.join(asset.name), b"invalid").unwrap();
        }
        assert!(verify_at(&root).is_err());
        let link = root.with_extension("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert!(verify_at(&link).is_err());
        std::fs::remove_file(link).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn explicit_calibration_pack_is_exact() {
        let Ok(directory) = std::env::var("DECK_TRANSLATION_CERT_PACK") else {
            return;
        };
        verify_at(Path::new(&directory)).unwrap();
        let root = std::env::temp_dir().join(format!(
            "deck-pack-integrity-{}-{}",
            std::process::id(),
            STAGE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        for asset in &ASSETS {
            std::fs::copy(
                Path::new(&directory).join(asset.name),
                root.join(asset.name),
            )
            .unwrap();
        }
        verify_at(&root).unwrap();
        let small = root.join(ASSETS[2].name);
        let mut bytes = std::fs::read(&small).unwrap();
        bytes[0] ^= 1;
        std::fs::write(&small, &bytes).unwrap();
        assert!(
            verify_at(&root).is_err(),
            "same-size changed asset must fail hash validation"
        );
        bytes.pop();
        std::fs::write(&small, &bytes).unwrap();
        assert!(
            verify_at(&root).is_err(),
            "truncated asset must fail size validation"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn explicit_isolated_download_certification() {
        let Ok(root) = std::env::var("DECK_TRANSLATION_CERT_DOWNLOAD_ROOT") else {
            return;
        };
        let directory = Path::new(&root).join("models/translation").join(PACK_ID);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let status = runtime.block_on(install_at(directory.clone())).unwrap();
        assert!(status.installed);
        verify_at(&directory).unwrap();
    }
}
