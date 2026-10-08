//! Content hashing.
//!
//! BLAKE3 is used both as the primary key of a document and as the basis of the
//! cache key, so it must be stable across runs and platforms. It only ever sees
//! the file's bytes: metadata such as mtime is recorded separately and never
//! contributes to the hash, so copying a file does not make it a new document.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// Hashes `bytes`, returning lowercase hex.
pub fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Hashes a file without loading it into memory.
///
/// Documents routinely run to hundreds of megabytes; buffering one whole to
/// hash it would be both wasteful and unnecessary.
pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut file = File::open(path)?;
    let mut buffer = vec![0u8; 64 * 1024];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_64_hex_characters() {
        let hash = content_hash(b"hello");
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn identical_bytes_hash_identically() {
        assert_eq!(content_hash(b"same"), content_hash(b"same"));
    }

    #[test]
    fn different_bytes_hash_differently() {
        assert_ne!(content_hash(b"a"), content_hash(b"b"));
        // Length differences must not be masked by padding.
        assert_ne!(content_hash(b"a"), content_hash(b"a "));
    }

    #[test]
    fn file_and_bytes_agree() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.bin");
        std::fs::write(&path, b"payload").unwrap();

        assert_eq!(hash_file(&path).unwrap(), content_hash(b"payload"));
    }

    #[test]
    fn hashing_a_missing_file_reports_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(hash_file(&dir.path().join("absent")).is_err());
    }

    /// Files larger than the read buffer must hash the same as their bytes,
    /// otherwise the streaming loop is dropping or duplicating a chunk.
    #[test]
    fn streaming_handles_files_larger_than_the_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.bin");
        let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &payload).unwrap();

        assert_eq!(hash_file(&path).unwrap(), content_hash(&payload));
    }
}
