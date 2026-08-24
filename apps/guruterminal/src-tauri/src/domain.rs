use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub use guruterminal_core::CanonicalMemoryKind;

mod chat;
mod guru;
mod memory;
mod memory_write;

pub use chat::*;
pub use guru::*;
pub use memory::*;
pub use memory_write::{MemoryChangeAuthority, MemoryChangeTarget, MemoryWrite};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DomainError {
    #[error("invalid domain value: {0}")]
    Invalid(&'static str),
}

pub(crate) fn sha256_hex(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hex_lower(&hasher.finalize())
}

pub(crate) fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn require_non_empty(value: &str, message: &'static str) -> Result<(), DomainError> {
    if value.trim().is_empty() {
        return Err(DomainError::Invalid(message));
    }
    Ok(())
}

fn require_bounded_text(
    value: &str,
    maximum: usize,
    message: &'static str,
) -> Result<(), DomainError> {
    if value.trim().is_empty() || value.len() > maximum || value.contains('\0') {
        return Err(DomainError::Invalid(message));
    }
    Ok(())
}

fn require_identifier(value: &str, message: &'static str) -> Result<(), DomainError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(DomainError::Invalid(message));
    }
    Ok(())
}

pub(crate) fn validate_canonical_memory_record_id(
    value: &str,
    expected_kind: Option<&str>,
) -> Result<(), DomainError> {
    let (kind, _) = CanonicalMemoryKind::parse_record_id(value)
        .ok_or(DomainError::Invalid("memory record id is invalid"))?;
    if expected_kind.is_some_and(|expected| expected != kind.slug()) {
        return Err(DomainError::Invalid("memory record id is invalid"));
    }
    Ok(())
}

fn validate_sha256_digest(value: &str, message: &'static str) -> Result<(), DomainError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DomainError::Invalid(message));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
