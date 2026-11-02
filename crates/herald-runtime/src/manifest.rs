use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::RuntimeError;

/// Everything needed to reproduce a model download.
///
/// The manifest is checked into the repo for development and used by the
/// downloader in later milestones. Model version changes are explicit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelManifest {
    pub name: String,
    pub repo: String,
    pub revision: String,
    pub filename: String,
    pub quantization: String,
    pub license: String,
    pub sha256: String,
    pub size_bytes: u64,
}

impl ModelManifest {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, RuntimeError> {
        let file = File::open(path)?;
        serde_json::from_reader(BufReader::new(file)).map_err(|error| {
            RuntimeError::InvalidConfig(format!("invalid model manifest: {error}"))
        })
    }

    /// Verify the file on disk against the pinned digest.
    pub fn verify(&self, path: impl AsRef<Path>) -> Result<(), RuntimeError> {
        verify_sha256(path, &self.sha256)
    }
}

/// Streaming SHA-256 so a multi-gigabyte model is never loaded into memory.
pub fn verify_sha256(path: impl AsRef<Path>, expected: &str) -> Result<(), RuntimeError> {
    let path = path.as_ref();
    let file = File::open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            RuntimeError::ModelMissing(path.to_path_buf())
        } else {
            RuntimeError::Io(error)
        }
    })?;

    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    let actual = format!("{:x}", hasher.finalize());
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(RuntimeError::ChecksumMismatch {
            path: path.to_path_buf(),
            expected: expected.to_string(),
            actual,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(contents: &[u8]) -> (std::path::PathBuf, std::fs::File) {
        let path = std::env::temp_dir().join(format!(
            "herald-checksum-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let mut file = File::create(&path).unwrap();
        file.write_all(contents).unwrap();
        file.flush().unwrap();
        (path, file)
    }

    #[test]
    fn accepts_matching_checksum() {
        let (path, _file) = temp_file(b"hello herald");
        let expected = format!("{:x}", Sha256::digest(b"hello herald"));
        verify_sha256(&path, &expected).unwrap();
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn rejects_mismatched_checksum() {
        let (path, _file) = temp_file(b"hello herald");
        let result = verify_sha256(&path, &"0".repeat(64));
        assert!(matches!(result, Err(RuntimeError::ChecksumMismatch { .. })));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn missing_model_is_reported_clearly() {
        let result = verify_sha256("/nonexistent/herald/model.gguf", "abc");
        assert!(matches!(result, Err(RuntimeError::ModelMissing(_))));
    }
}
