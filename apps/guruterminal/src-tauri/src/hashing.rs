use sha2::{Digest, Sha256};

/// Lowercase hex SHA-256 digest of raw bytes, shared by every content-digest surface.
pub fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
