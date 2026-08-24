use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

const CREDENTIAL_SCHEMA_VERSION: &str = "guruterminal-finance-credential/2";

#[cfg(debug_assertions)]
#[cfg_attr(any(test, feature = "webdriver"), allow(dead_code))]
const NATIVE_SERVICE: &str = "com.monarchjuno.guruterminal.finance.development";
#[cfg(not(debug_assertions))]
#[cfg_attr(any(test, feature = "webdriver"), allow(dead_code))]
const NATIVE_SERVICE: &str = "com.monarchjuno.guruterminal.finance";

#[derive(Debug, Error)]
pub enum FinanceCredentialError {
    #[error("the native credential store is unavailable on this platform")]
    Unsupported,
    #[error("the native credential store operation failed")]
    Store,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateVerification {
    Never,
    Rejected,
    TemporarilyUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationOutcome {
    Verified,
    Rejected,
    TemporarilyUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinishVerification {
    Applied,
    Stale,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialStatus {
    pub stored: bool,
    pub active: bool,
    pub pending: bool,
    pub active_fields: BTreeSet<String>,
    pub candidate_fields: BTreeSet<String>,
    pub verification: CredentialVerification,
    pub verified_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialVerification {
    Never,
    Verified,
    Rejected,
    TemporarilyUnavailable,
}

/// A snapshot of the exact staged candidate that a caller may verify.
///
/// This type intentionally has no `Debug` or serialization implementation so
/// the secret cannot accidentally cross the command boundary or enter logs.
pub struct CredentialCandidate {
    revision: String,
    secrets: BTreeMap<String, String>,
}

impl CredentialCandidate {
    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn secrets(&self) -> &BTreeMap<String, String> {
        &self.secrets
    }

    pub fn get(&self, credential_id: &str) -> Option<&str> {
        self.secrets.get(credential_id).map(String::as_str)
    }
}

/// A verified credential bundle that finance execution may use.
///
/// This type intentionally has no `Debug` or serialization implementation so
/// secrets cannot accidentally cross the command boundary or enter logs.
pub struct ActiveCredentialBundle {
    revision: String,
    secrets: BTreeMap<String, String>,
}

impl ActiveCredentialBundle {
    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn get(&self, credential_id: &str) -> Option<&str> {
        self.secrets.get(credential_id).map(String::as_str)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredCredentialBundle {
    revision: String,
    secrets: BTreeMap<String, String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ActiveCredential {
    credential: StoredCredentialBundle,
    verified_at: i64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CandidateCredential {
    credential: StoredCredentialBundle,
    verification: CandidateVerification,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CredentialEnvelope {
    schema_version: String,
    active: Option<ActiveCredential>,
    candidate: Option<CandidateCredential>,
}

impl Default for CredentialEnvelope {
    fn default() -> Self {
        Self {
            schema_version: CREDENTIAL_SCHEMA_VERSION.to_owned(),
            active: None,
            candidate: None,
        }
    }
}

fn validate_entry_id(entry_id: &str) -> Result<(), FinanceCredentialError> {
    if entry_id.is_empty()
        || entry_id.len() > 96
        || !entry_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte))
    {
        return Err(FinanceCredentialError::Store);
    }
    Ok(())
}

fn validate_credential_id(credential_id: &str) -> Result<(), FinanceCredentialError> {
    if credential_id.is_empty()
        || credential_id.len() > 64
        || !credential_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        return Err(FinanceCredentialError::Store);
    }
    Ok(())
}

fn validate_secret(secret: &str) -> Result<(), FinanceCredentialError> {
    if secret.is_empty()
        || secret.len() > 512
        || secret.contains('\0')
        || secret.chars().any(|character| character.is_control())
    {
        return Err(FinanceCredentialError::Store);
    }
    Ok(())
}

fn validate_secrets(secrets: &BTreeMap<String, String>) -> Result<(), FinanceCredentialError> {
    if secrets.is_empty() || secrets.len() > 16 {
        return Err(FinanceCredentialError::Store);
    }
    for (credential_id, secret) in secrets {
        validate_credential_id(credential_id)?;
        validate_secret(secret)?;
    }
    Ok(())
}

fn validate_revision(revision: &str) -> Result<(), FinanceCredentialError> {
    let parsed = Uuid::parse_str(revision).map_err(|_| FinanceCredentialError::Store)?;
    if parsed.hyphenated().to_string() != revision {
        return Err(FinanceCredentialError::Store);
    }
    Ok(())
}

fn validate_envelope(envelope: &CredentialEnvelope) -> Result<(), FinanceCredentialError> {
    if envelope.schema_version != CREDENTIAL_SCHEMA_VERSION {
        return Err(FinanceCredentialError::Store);
    }
    if let Some(active) = &envelope.active {
        validate_revision(&active.credential.revision)?;
        validate_secrets(&active.credential.secrets)?;
        if active.verified_at < 0 {
            return Err(FinanceCredentialError::Store);
        }
    }
    if let Some(candidate) = &envelope.candidate {
        validate_revision(&candidate.credential.revision)?;
        validate_secrets(&candidate.credential.secrets)?;
    }
    Ok(())
}

fn operation_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(Default::default)
}

#[cfg(any(test, feature = "webdriver"))]
fn memory_store() -> &'static std::sync::Mutex<std::collections::BTreeMap<String, String>> {
    static STORE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    > = std::sync::OnceLock::new();
    STORE.get_or_init(Default::default)
}

fn native_account(entry_id: &str) -> String {
    format!("credential-state/v2/{entry_id}")
}

#[cfg(any(test, feature = "webdriver"))]
fn read_memory_blob(entry_id: &str) -> Result<Option<String>, FinanceCredentialError> {
    Ok(memory_store()
        .lock()
        .map_err(|_| FinanceCredentialError::Store)?
        .get(&native_account(entry_id))
        .cloned())
}

#[cfg(any(test, feature = "webdriver"))]
fn write_memory_blob(entry_id: &str, value: &str) -> Result<(), FinanceCredentialError> {
    memory_store()
        .lock()
        .map_err(|_| FinanceCredentialError::Store)?
        .insert(native_account(entry_id), value.to_owned());
    Ok(())
}

#[cfg(any(test, feature = "webdriver"))]
fn delete_memory_blob(entry_id: &str) -> Result<(), FinanceCredentialError> {
    memory_store()
        .lock()
        .map_err(|_| FinanceCredentialError::Store)?
        .remove(&native_account(entry_id));
    Ok(())
}

#[cfg(all(
    not(any(test, feature = "webdriver")),
    any(target_os = "macos", windows),
))]
fn native_entry(service: &str, entry_id: &str) -> Result<keyring::Entry, FinanceCredentialError> {
    keyring::Entry::new(service, &native_account(entry_id))
        .map_err(|_| FinanceCredentialError::Store)
}

#[cfg(all(
    not(any(test, feature = "webdriver")),
    any(target_os = "macos", windows),
))]
fn read_native_blob(
    service: &str,
    entry_id: &str,
) -> Result<Option<String>, FinanceCredentialError> {
    match native_entry(service, entry_id)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err(FinanceCredentialError::Store),
    }
}

#[cfg(all(
    not(any(test, feature = "webdriver")),
    any(target_os = "macos", windows),
))]
fn write_native_blob(
    service: &str,
    entry_id: &str,
    value: &str,
) -> Result<(), FinanceCredentialError> {
    native_entry(service, entry_id)?
        .set_password(value)
        .map_err(|_| FinanceCredentialError::Store)
}

#[cfg(all(
    not(any(test, feature = "webdriver")),
    any(target_os = "macos", windows),
))]
fn delete_native_blob(service: &str, entry_id: &str) -> Result<(), FinanceCredentialError> {
    match native_entry(service, entry_id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(FinanceCredentialError::Store),
    }
}

#[cfg(any(test, feature = "webdriver"))]
fn read_blob(entry_id: &str) -> Result<Option<String>, FinanceCredentialError> {
    read_memory_blob(entry_id)
}

#[cfg(any(test, feature = "webdriver"))]
fn write_blob(entry_id: &str, value: &str) -> Result<(), FinanceCredentialError> {
    write_memory_blob(entry_id, value)
}

#[cfg(any(test, feature = "webdriver"))]
fn delete_blob(entry_id: &str) -> Result<(), FinanceCredentialError> {
    delete_memory_blob(entry_id)
}

#[cfg(all(
    not(any(test, feature = "webdriver")),
    any(target_os = "macos", windows)
))]
fn read_blob(entry_id: &str) -> Result<Option<String>, FinanceCredentialError> {
    read_native_blob(NATIVE_SERVICE, entry_id)
}

#[cfg(all(
    not(any(test, feature = "webdriver")),
    any(target_os = "macos", windows)
))]
fn write_blob(entry_id: &str, value: &str) -> Result<(), FinanceCredentialError> {
    write_native_blob(NATIVE_SERVICE, entry_id, value)
}

#[cfg(all(
    not(any(test, feature = "webdriver")),
    any(target_os = "macos", windows)
))]
fn delete_blob(entry_id: &str) -> Result<(), FinanceCredentialError> {
    delete_native_blob(NATIVE_SERVICE, entry_id)
}

#[cfg(all(
    not(any(test, feature = "webdriver")),
    not(any(target_os = "macos", windows))
))]
fn read_blob(_entry_id: &str) -> Result<Option<String>, FinanceCredentialError> {
    Err(FinanceCredentialError::Unsupported)
}

#[cfg(all(
    not(any(test, feature = "webdriver")),
    not(any(target_os = "macos", windows))
))]
fn write_blob(_entry_id: &str, _value: &str) -> Result<(), FinanceCredentialError> {
    Err(FinanceCredentialError::Unsupported)
}

#[cfg(all(
    not(any(test, feature = "webdriver")),
    not(any(target_os = "macos", windows))
))]
fn delete_blob(_entry_id: &str) -> Result<(), FinanceCredentialError> {
    Err(FinanceCredentialError::Unsupported)
}

fn load_locked(entry_id: &str) -> Result<CredentialEnvelope, FinanceCredentialError> {
    validate_entry_id(entry_id)?;
    let Some(value) = read_blob(entry_id)? else {
        return Ok(CredentialEnvelope::default());
    };
    let envelope: CredentialEnvelope =
        serde_json::from_str(&value).map_err(|_| FinanceCredentialError::Store)?;
    validate_envelope(&envelope)?;
    Ok(envelope)
}

fn save_locked(
    entry_id: &str,
    envelope: &CredentialEnvelope,
) -> Result<(), FinanceCredentialError> {
    validate_envelope(envelope)?;
    let value = serde_json::to_string(envelope).map_err(|_| FinanceCredentialError::Store)?;
    write_blob(entry_id, &value)
}

fn status_from(envelope: &CredentialEnvelope) -> CredentialStatus {
    let active_fields = envelope
        .active
        .as_ref()
        .map(|active| active.credential.secrets.keys().cloned().collect())
        .unwrap_or_default();
    let candidate_fields = envelope
        .candidate
        .as_ref()
        .map(|candidate| candidate.credential.secrets.keys().cloned().collect())
        .unwrap_or_default();
    let verification = envelope
        .candidate
        .as_ref()
        .map(|candidate| match candidate.verification {
            CandidateVerification::Never => CredentialVerification::Never,
            CandidateVerification::Rejected => CredentialVerification::Rejected,
            CandidateVerification::TemporarilyUnavailable => {
                CredentialVerification::TemporarilyUnavailable
            }
        })
        .unwrap_or_else(|| {
            if envelope.active.is_some() {
                CredentialVerification::Verified
            } else {
                CredentialVerification::Never
            }
        });
    CredentialStatus {
        stored: envelope.active.is_some() || envelope.candidate.is_some(),
        active: envelope.active.is_some(),
        pending: envelope.candidate.is_some(),
        active_fields,
        candidate_fields,
        verification,
        verified_at: envelope
            .active
            .as_ref()
            .map(|credential| credential.verified_at),
    }
}

/// Atomically stages a partial credential patch without changing the active
/// bundle. Existing active values seed the candidate, an existing candidate
/// takes precedence over them, and the submitted patch wins last. This lets a
/// native caller update one profile field without asking React to resubmit or
/// observe any previously stored secret.
pub fn stage(
    entry_id: &str,
    patch: &BTreeMap<String, String>,
) -> Result<CredentialStatus, FinanceCredentialError> {
    validate_entry_id(entry_id)?;
    validate_secrets(patch)?;
    let _guard = operation_lock()
        .lock()
        .map_err(|_| FinanceCredentialError::Store)?;
    let mut envelope = load_locked(entry_id)?;
    let mut secrets = envelope
        .active
        .as_ref()
        .map(|active| active.credential.secrets.clone())
        .unwrap_or_default();
    if let Some(candidate) = &envelope.candidate {
        secrets.extend(candidate.credential.secrets.clone());
    }
    secrets.extend(patch.clone());
    validate_secrets(&secrets)?;
    envelope.candidate = Some(CandidateCredential {
        credential: StoredCredentialBundle {
            revision: Uuid::new_v4().hyphenated().to_string(),
            secrets,
        },
        verification: CandidateVerification::Never,
    });
    save_locked(entry_id, &envelope)?;
    Ok(status_from(&envelope))
}

/// Loads only the current candidate. An active credential is never returned by
/// this function and therefore cannot be confused with an unverified replacement.
pub fn candidate(entry_id: &str) -> Result<Option<CredentialCandidate>, FinanceCredentialError> {
    let _guard = operation_lock()
        .lock()
        .map_err(|_| FinanceCredentialError::Store)?;
    Ok(load_locked(entry_id)?
        .candidate
        .map(|candidate| CredentialCandidate {
            revision: candidate.credential.revision,
            secrets: candidate.credential.secrets,
        }))
}

/// Applies a verification result only when `expected_revision` is still the
/// staged candidate. This compare-and-swap prevents a slow verification from
/// promoting or rejecting a newer replacement.
pub fn finish_verification(
    entry_id: &str,
    expected_revision: &str,
    outcome: VerificationOutcome,
    verified_at: i64,
) -> Result<FinishVerification, FinanceCredentialError> {
    validate_revision(expected_revision)?;
    if verified_at < 0 {
        return Err(FinanceCredentialError::Store);
    }
    let _guard = operation_lock()
        .lock()
        .map_err(|_| FinanceCredentialError::Store)?;
    let mut envelope = load_locked(entry_id)?;
    let Some(candidate) = envelope.candidate.as_mut() else {
        return Ok(FinishVerification::Stale);
    };
    if candidate.credential.revision != expected_revision {
        return Ok(FinishVerification::Stale);
    }
    match outcome {
        VerificationOutcome::Verified => {
            let candidate = envelope
                .candidate
                .take()
                .expect("candidate was checked above");
            envelope.active = Some(ActiveCredential {
                credential: candidate.credential,
                verified_at,
            });
        }
        VerificationOutcome::Rejected => {
            candidate.verification = CandidateVerification::Rejected;
        }
        VerificationOutcome::TemporarilyUnavailable => {
            candidate.verification = CandidateVerification::TemporarilyUnavailable;
        }
    }
    save_locked(entry_id, &envelope)?;
    Ok(FinishVerification::Applied)
}

pub fn status(entry_id: &str) -> Result<CredentialStatus, FinanceCredentialError> {
    let _guard = operation_lock()
        .lock()
        .map_err(|_| FinanceCredentialError::Store)?;
    Ok(status_from(&load_locked(entry_id)?))
}

/// Returns only a verified active bundle. Staged or rejected values are never
/// visible to finance execution.
pub fn get(entry_id: &str) -> Result<Option<ActiveCredentialBundle>, FinanceCredentialError> {
    let _guard = operation_lock()
        .lock()
        .map_err(|_| FinanceCredentialError::Store)?;
    Ok(load_locked(entry_id)?
        .active
        .map(|active| ActiveCredentialBundle {
            revision: active.credential.revision,
            secrets: active.credential.secrets,
        }))
}

pub fn has_active(entry_id: &str) -> Result<bool, FinanceCredentialError> {
    status(entry_id).map(|status| status.active)
}

pub fn delete(entry_id: &str) -> Result<(), FinanceCredentialError> {
    validate_entry_id(entry_id)?;
    let _guard = operation_lock()
        .lock()
        .map_err(|_| FinanceCredentialError::Store)?;
    delete_blob(entry_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(values: &[(&str, &str)]) -> BTreeMap<String, String> {
        values
            .iter()
            .map(|(credential_id, secret)| ((*credential_id).to_owned(), (*secret).to_owned()))
            .collect()
    }

    fn clean(entry_id: &str) {
        delete(entry_id).unwrap();
    }

    fn promote(entry_id: &str, secrets: &BTreeMap<String, String>, verified_at: i64) {
        stage(entry_id, secrets).unwrap();
        let candidate = candidate(entry_id).unwrap().unwrap();
        assert_eq!(
            finish_verification(
                entry_id,
                candidate.revision(),
                VerificationOutcome::Verified,
                verified_at,
            )
            .unwrap(),
            FinishVerification::Applied
        );
    }

    #[test]
    fn staged_credentials_are_not_executable_until_exact_revision_is_verified() {
        let entry = "test.stage-promote";
        clean(entry);
        let secrets = bundle(&[
            ("app_key", "candidate-app-key"),
            ("app_secret", "candidate-app-secret"),
        ]);
        let staged_status = stage(entry, &secrets).unwrap();
        assert!(staged_status.stored);
        assert!(!staged_status.active);
        assert!(staged_status.pending);
        assert!(get(entry).unwrap().is_none());

        let candidate = candidate(entry).unwrap().unwrap();
        assert_eq!(candidate.get("app_key"), Some("candidate-app-key"));
        assert_eq!(candidate.get("app_secret"), Some("candidate-app-secret"));
        assert_eq!(
            finish_verification(
                entry,
                candidate.revision(),
                VerificationOutcome::Verified,
                17,
            )
            .unwrap(),
            FinishVerification::Applied
        );
        let active = get(entry).unwrap().unwrap();
        assert_eq!(active.revision(), candidate.revision());
        assert_eq!(active.get("app_key"), Some("candidate-app-key"));
        assert_eq!(active.get("app_secret"), Some("candidate-app-secret"));
        let status = status(entry).unwrap();
        assert!(status.active);
        assert!(!status.pending);
        assert_eq!(status.verification, CredentialVerification::Verified);
        assert_eq!(status.verified_at, Some(17));
        clean(entry);
    }

    #[test]
    fn stale_verification_cannot_overwrite_a_newer_candidate_or_active_secret() {
        let entry = "test.stale-cas";
        clean(entry);
        promote(entry, &bundle(&[("api_key", "known-good-secret")]), 10);

        stage(entry, &bundle(&[("api_key", "slow-candidate-secret")])).unwrap();
        let slow = candidate(entry).unwrap().unwrap();
        stage(entry, &bundle(&[("api_key", "newer-candidate-secret")])).unwrap();
        let newer = candidate(entry).unwrap().unwrap();
        assert_ne!(slow.revision(), newer.revision());

        assert_eq!(
            finish_verification(entry, slow.revision(), VerificationOutcome::Verified, 20,)
                .unwrap(),
            FinishVerification::Stale
        );
        let active = get(entry).unwrap().unwrap();
        assert_eq!(active.get("api_key"), Some("known-good-secret"));
        let current = candidate(entry).unwrap().unwrap();
        assert_eq!(current.revision(), newer.revision());
        assert_eq!(current.get("api_key"), Some("newer-candidate-secret"));
        clean(entry);
    }

    #[test]
    fn failed_replacement_retains_the_verified_active_revision() {
        let entry = "test.failed-replacement";
        clean(entry);
        promote(
            entry,
            &bundle(&[
                ("app_key", "known-good-key"),
                ("app_secret", "known-good-secret"),
            ]),
            10,
        );
        stage(
            entry,
            &bundle(&[
                ("app_key", "rejected-new-key"),
                ("app_secret", "rejected-new-secret"),
            ]),
        )
        .unwrap();
        let replacement = candidate(entry).unwrap().unwrap();
        assert_eq!(
            finish_verification(
                entry,
                replacement.revision(),
                VerificationOutcome::Rejected,
                20,
            )
            .unwrap(),
            FinishVerification::Applied
        );
        let active = get(entry).unwrap().unwrap();
        assert_eq!(active.get("app_key"), Some("known-good-key"));
        assert_eq!(active.get("app_secret"), Some("known-good-secret"));
        let status = status(entry).unwrap();
        assert!(status.active);
        assert!(status.pending);
        assert_eq!(status.verification, CredentialVerification::Rejected);
        assert_eq!(status.verified_at, Some(10));
        clean(entry);
    }

    #[test]
    fn partial_patch_merges_active_and_candidate_fields_atomically() {
        let entry = "test.partial-profile";
        clean(entry);
        promote(
            entry,
            &bundle(&[
                ("app_key", "known-good-key"),
                ("app_secret", "known-good-secret"),
            ]),
            10,
        );

        let staged = stage(entry, &bundle(&[("account_product_code", "01")])).unwrap();
        assert_eq!(
            staged.active_fields,
            BTreeSet::from(["app_key".to_owned(), "app_secret".to_owned()])
        );
        assert_eq!(
            staged.candidate_fields,
            BTreeSet::from([
                "account_product_code".to_owned(),
                "app_key".to_owned(),
                "app_secret".to_owned(),
            ])
        );

        stage(entry, &bundle(&[("account_number", "12345678")])).unwrap();
        let candidate = candidate(entry).unwrap().unwrap();
        assert_eq!(candidate.get("app_key"), Some("known-good-key"));
        assert_eq!(candidate.get("app_secret"), Some("known-good-secret"));
        assert_eq!(candidate.get("account_number"), Some("12345678"));
        assert_eq!(candidate.get("account_product_code"), Some("01"));
        clean(entry);
    }

    #[test]
    fn verified_revision_is_reloaded_from_the_store() {
        let entry = "test.restart-status";
        clean(entry);
        promote(entry, &bundle(&[("api_key", "restart-safe-secret")]), 42);
        let persisted = memory_store()
            .lock()
            .unwrap()
            .get(&native_account(entry))
            .cloned()
            .unwrap();
        assert!(persisted.contains(CREDENTIAL_SCHEMA_VERSION));

        let reloaded = status(entry).unwrap();
        assert!(reloaded.active);
        assert!(!reloaded.pending);
        assert_eq!(reloaded.verification, CredentialVerification::Verified);
        assert_eq!(reloaded.verified_at, Some(42));
        clean(entry);
    }

    #[test]
    fn restart_status_never_upgrades_an_unverified_candidate() {
        let entry = "test.restart-unverified";
        clean(entry);
        stage(entry, &bundle(&[("api_key", "unverified-restart-secret")])).unwrap();
        let persisted = memory_store()
            .lock()
            .unwrap()
            .get(&native_account(entry))
            .cloned()
            .unwrap();
        assert!(persisted.contains(CREDENTIAL_SCHEMA_VERSION));

        let reloaded = status(entry).unwrap();
        assert!(reloaded.stored);
        assert!(!reloaded.active);
        assert!(reloaded.pending);
        assert_eq!(reloaded.verification, CredentialVerification::Never);
        assert_eq!(reloaded.verified_at, None);
        assert!(get(entry).unwrap().is_none());
        clean(entry);
    }

    #[test]
    fn delete_removes_active_and_candidate_material() {
        let entry = "test.delete-all";
        clean(entry);
        promote(entry, &bundle(&[("api_key", "active-secret-value")]), 10);
        stage(entry, &bundle(&[("api_key", "pending-secret-value")])).unwrap();
        delete(entry).unwrap();
        assert!(get(entry).unwrap().is_none());
        assert_eq!(candidate(entry).unwrap().map(|_| ()), None);
        assert!(!status(entry).unwrap().stored);
    }

    #[test]
    fn development_keyring_namespace_cannot_collide_with_installed_app() {
        assert_eq!(
            native_account("fred.macro"),
            "credential-state/v2/fred.macro"
        );
        if cfg!(debug_assertions) {
            assert_eq!(
                NATIVE_SERVICE,
                "com.monarchjuno.guruterminal.finance.development"
            );
            assert_ne!(NATIVE_SERVICE, "com.monarchjuno.guruterminal.finance");
        }
    }

    #[test]
    fn unsafe_identifiers_and_secrets_are_rejected() {
        assert!(stage(
            "../../credential",
            &bundle(&[("api_key", "test-secret-value")]),
        )
        .is_err());
        assert!(stage("fred.macro", &bundle(&[("api_key", "line\nbreak-secret")]),).is_err());
        assert!(stage("fred.macro", &bundle(&[("../../key", "test-secret-value")]),).is_err());
        assert!(stage("fred.macro", &BTreeMap::new()).is_err());
    }
}
