//! Bounded durable state documents: the mechanism shared by the MCP ledger
//! (`mcp/state.rs`), the Phone Connector journal (`connector/journal.rs`) and the
//! channel inbox (`inbound_channel.rs`). Those owners share nothing else — no
//! record type, token, grant or authority — so this module knows no document
//! shape; it takes a byte bound and a label. A load reads at most the bound
//! plus one byte and reports `Recovery` for an oversized or undecodable file
//! instead of parsing it; an absent file is `None` and the owner decides what
//! a fresh document is (the owner still validates every loaded record). A
//! write refuses to exceed the bound (`DiskFull`) before the bytes reach
//! `datadir::atomic_write`. Ids come from the one secure random source and
//! hashes from SHA-256 in lowercase hex. Messages are
//! "`<label>` could not be read / exceeds its bounds / is unreadable /
//! encoding failed / capacity reached", so each owner keeps its own wording
//! through the label only.

use ring::rand::{SecureRandom, SystemRandom};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

use crate::error::{DeckError, ErrorKind};

/// Lowercase hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// SHA-256 of `bytes` as lowercase hex.
pub(crate) fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// `bytes` secure random bytes.
pub(crate) fn random(bytes: usize) -> Result<Vec<u8>, DeckError> {
    let mut out = vec![0; bytes];
    SystemRandom::new()
        .fill(&mut out)
        .map_err(|_| DeckError::new(ErrorKind::Other, "secure random unavailable"))?;
    Ok(out)
}

/// `prefix` followed by `bytes` secure random bytes in hex.
pub(crate) fn random_id(prefix: &str, bytes: usize) -> Result<String, DeckError> {
    Ok(format!("{prefix}{}", hex(&random(bytes)?)))
}

/// The bytes at `path`, or `None` when the file does not exist. A file over
/// `max` bytes is reported, never returned.
pub(crate) fn read_bounded(
    path: &Path,
    max: usize,
    label: &str,
) -> Result<Option<Vec<u8>>, DeckError> {
    let mut bytes = Vec::new();
    match std::fs::File::open(path) {
        Ok(file) => {
            file.take(max as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| {
                    DeckError::new(
                        ErrorKind::io(error.kind()),
                        format!("{label} could not be read"),
                    )
                })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(DeckError::new(
                ErrorKind::io(error.kind()),
                format!("{label} could not be read"),
            ));
        }
    }
    if bytes.len() > max {
        return Err(DeckError::new(
            ErrorKind::Recovery,
            format!("{label} exceeds its bounds"),
        ));
    }
    Ok(Some(bytes))
}

/// `bytes` decoded as a document.
pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8], label: &str) -> Result<T, DeckError> {
    serde_json::from_slice(bytes)
        .map_err(|_| DeckError::new(ErrorKind::Recovery, format!("{label} is unreadable")))
}

/// `read_bounded` then `decode`.
pub(crate) fn load_bounded<T: DeserializeOwned>(
    path: &Path,
    max: usize,
    label: &str,
) -> Result<Option<T>, DeckError> {
    read_bounded(path, max, label)?
        .map(|bytes| decode(&bytes, label))
        .transpose()
}

/// `doc` as JSON bytes.
pub(crate) fn encode<T: Serialize>(doc: &T, label: &str) -> Result<Vec<u8>, DeckError> {
    serde_json::to_vec(doc)
        .map_err(|_| DeckError::new(ErrorKind::Other, format!("{label} encoding failed")))
}

/// Atomically replace `path` with `bytes` unless they exceed `max`.
pub(crate) fn write_bounded(
    path: &Path,
    bytes: &[u8],
    max: usize,
    label: &str,
) -> Result<(), DeckError> {
    if bytes.len() > max {
        return Err(DeckError::new(
            ErrorKind::DiskFull,
            format!("{label} capacity reached"),
        ));
    }
    crate::datadir::atomic_write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("deck-ledger-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn ids_and_hashes_are_hex() {
        let id = random_id("op_", 16).unwrap();
        assert_eq!(id.len(), 3 + 32);
        assert!(id[3..].bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(id, random_id("op_", 16).unwrap());
        assert_eq!(
            sha(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(hex(&[0, 255]), "00ff");
    }

    #[test]
    fn absent_document_is_none_and_round_trips_within_the_bound() {
        let path = tdir("roundtrip").join("doc.json");
        assert!(load_bounded::<Vec<u32>>(&path, 64, "doc")
            .unwrap()
            .is_none());
        let bytes = encode(&vec![1u32, 2, 3], "doc").unwrap();
        write_bounded(&path, &bytes, 64, "doc").unwrap();
        assert_eq!(
            load_bounded::<Vec<u32>>(&path, 64, "doc").unwrap(),
            Some(vec![1, 2, 3])
        );
    }

    #[test]
    fn oversized_and_undecodable_files_report_recovery_without_decoding() {
        let path = tdir("oversize").join("doc.json");
        std::fs::write(&path, b"[1,2,3]").unwrap();
        let error = load_bounded::<Vec<u32>>(&path, 4, "the doc").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Recovery);
        assert_eq!(error.message(), "the doc exceeds its bounds");
        std::fs::write(&path, b"nope").unwrap();
        let error = load_bounded::<Vec<u32>>(&path, 64, "the doc").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Recovery);
        assert_eq!(error.message(), "the doc is unreadable");
    }

    #[test]
    fn a_write_over_the_bound_is_refused_before_touching_the_file() {
        let path = tdir("cap").join("doc.json");
        let error = write_bounded(&path, b"12345", 4, "the doc").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::DiskFull);
        assert_eq!(error.message(), "the doc capacity reached");
        assert!(!path.exists());
    }
}
