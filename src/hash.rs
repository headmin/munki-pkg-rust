//! SHA-256 helpers for build manifests and provenance.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Read size for streaming file hashes. Keeps a large package out of memory.
const CHUNK: usize = 1 << 20;

/// Lowercase hex encoding of a digest.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Streaming SHA-256 of a file, as lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .with_context(|| format!("Failed to open {} to hash it", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];

    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(to_hex(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// The canonical SHA-256 of the empty string.
    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn test_hex_is_lowercase_and_padded() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xff]), "000fff");
    }

    #[test]
    fn test_empty_file() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("empty");
        fs::write(&path, b"").unwrap();

        assert_eq!(sha256_file(&path).unwrap(), EMPTY);
    }

    #[test]
    fn test_known_digest() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("abc");
        fs::write(&path, b"abc").unwrap();

        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// Exercises the chunking loop with a payload larger than one read.
    #[test]
    fn test_multi_chunk_file_matches_single_shot() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("big");
        let data = vec![0x5au8; CHUNK * 2 + 17];
        fs::write(&path, &data).unwrap();

        let expected = to_hex(&Sha256::digest(&data));
        assert_eq!(sha256_file(&path).unwrap(), expected);
    }

    #[test]
    fn test_missing_file_is_an_error() {
        let temp = TempDir::new().unwrap();
        assert!(sha256_file(&temp.path().join("nope")).is_err());
    }
}
