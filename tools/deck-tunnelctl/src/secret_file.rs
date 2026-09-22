use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub struct SecretFile {
    path: PathBuf,
    directory: PathBuf,
}

impl SecretFile {
    pub fn create(secret: &[u8]) -> Result<Self, &'static str> {
        let base = std::env::temp_dir();
        let nonce = format!("{}-{}", std::process::id(), random_suffix()?);
        let directory = base.join(format!("deck-tunnelctl-{nonce}"));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|_| "secret_create_failed")?;
        let directory_metadata =
            fs::symlink_metadata(&directory).map_err(|_| "secret_validate_failed")?;
        if !directory_metadata.is_dir()
            || directory_metadata.file_type().is_symlink()
            || directory_metadata.permissions().mode() & 0o777 != 0o700
            || directory_metadata.uid() != unsafe { libc::geteuid() }
        {
            let _ = fs::remove_dir(&directory);
            return Err("secret_validate_failed");
        }
        let path = directory.join("runtime-key");
        let result = (|| {
            let mut file = open_new(&path)?;
            file.write_all(secret).map_err(|_| "secret_write_failed")?;
            file.sync_all().map_err(|_| "secret_write_failed")?;
            let metadata = file.metadata().map_err(|_| "secret_validate_failed")?;
            if !metadata.is_file()
                || metadata.permissions().mode() & 0o777 != 0o600
                || metadata.uid() != unsafe { libc::geteuid() }
            {
                return Err("secret_validate_failed");
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_dir(&directory);
            return Err(error);
        }
        Ok(Self { path, directory })
    }

    pub fn reference(&self) -> String {
        format!("file:{}", self.path.to_string_lossy())
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn open_new(path: &Path) -> Result<File, &'static str> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "secret_create_failed")
}

fn random_suffix() -> Result<String, &'static str> {
    let mut bytes = [0u8; 16];
    let result = unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) };
    if result != 0 {
        return Err("random_unavailable");
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

impl Drop for SecretFile {
    fn drop(&mut self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.path) {
            if metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
                let _ = fs::remove_file(&self.path);
            }
        }
        let _ = fs::remove_dir(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn secret_is_private_and_removed_on_drop() {
        let path;
        let directory;
        {
            let secret = SecretFile::create(b"do-not-log-this").unwrap();
            path = secret.path.clone();
            directory = secret.directory.clone();
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(metadata.is_file());
            assert_eq!(metadata.mode() & 0o777, 0o600);
            assert_eq!(
                fs::symlink_metadata(&directory).unwrap().mode() & 0o777,
                0o700
            );
            assert_eq!(secret.reference(), format!("file:{}", path.display()));
        }
        assert!(!path.exists());
        assert!(!directory.exists());
    }
}
