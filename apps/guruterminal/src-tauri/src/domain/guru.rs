use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{
    require_bounded_text, require_identifier, require_non_empty, required_option, DomainError,
};
use crate::settings::valid_model_profile_id;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RootFilesystemIdentity {
    pub device: u64,
    pub inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuruProfile {
    pub id: String,
    pub name: String,
    pub description: String,
    pub storage_kind: GuruStorageKind,
    pub memory_root: String,
    #[serde(deserialize_with = "required_option")]
    pub root_filesystem_identity: Option<RootFilesystemIdentity>,
    #[serde(deserialize_with = "required_option")]
    pub last_model_profile_id: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl GuruProfile {
    pub fn validate(&self) -> Result<(), DomainError> {
        require_identifier(&self.id, "guru id is empty or unsafe")?;
        require_non_empty(&self.name, "guru name is empty")?;
        if self.name.len() > 80 || self.name.chars().any(char::is_control) {
            return Err(DomainError::Invalid("guru name is invalid"));
        }
        require_non_empty(&self.memory_root, "guru memory root is empty")?;
        if self.storage_kind != GuruStorageKind::Managed {
            return Err(DomainError::Invalid("Guru storage kind is unsupported"));
        }
        if self
            .last_model_profile_id
            .as_deref()
            .is_some_and(|value| !valid_model_profile_id(value))
        {
            return Err(DomainError::Invalid("last model profile id is invalid"));
        }
        if self.created_at_ms < 0 || self.updated_at_ms < self.created_at_ms {
            return Err(DomainError::Invalid("guru timestamps are invalid"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuruStorageKind {
    Managed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuruCapabilityBinding {
    pub guru_id: String,
    pub entry_id: String,
    pub enabled: bool,
    pub granted_permissions: Vec<String>,
    pub config: std::collections::BTreeMap<String, String>,
    pub updated_at_ms: i64,
}

impl GuruCapabilityBinding {
    pub fn validate(&self) -> Result<(), DomainError> {
        require_identifier(&self.guru_id, "capability Guru id is invalid")?;
        if self.entry_id.is_empty()
            || self.entry_id.len() > 96
            || !self.entry_id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte)
            })
        {
            return Err(DomainError::Invalid("capability entry id is invalid"));
        }
        if self.granted_permissions.len() > 32 || self.config.len() > 32 {
            return Err(DomainError::Invalid("capability settings are too large"));
        }
        let mut permissions = BTreeSet::new();
        for permission in &self.granted_permissions {
            require_bounded_text(permission, 256, "capability permission is invalid")?;
            if !permissions.insert(permission.as_str()) {
                return Err(DomainError::Invalid(
                    "capability permissions contain duplicates",
                ));
            }
        }
        for (key, value) in &self.config {
            require_bounded_text(key, 128, "capability config key is invalid")?;
            if value.len() > 4_096 || value.contains('\0') {
                return Err(DomainError::Invalid("capability config value is invalid"));
            }
        }
        if self.updated_at_ms < 0 {
            return Err(DomainError::Invalid("capability timestamp is invalid"));
        }
        Ok(())
    }
}

pub fn default_guru_capability_bindings(
    guru_id: &str,
    timestamp: i64,
) -> Vec<GuruCapabilityBinding> {
    let catalog = crate::marketplace::bundled_catalog()
        .expect("the bundled Marketplace catalog is a build invariant");
    let mut bindings = catalog
        .entries
        .into_iter()
        .map(|entry| {
            let enabled = entry.setup.as_ref().is_none_or(|setup| {
                setup.config_fields.iter().all(|field| !field.required)
                    && setup.credential_fields.iter().all(|field| !field.required)
            });
            GuruCapabilityBinding {
                guru_id: guru_id.to_owned(),
                entry_id: entry.id,
                enabled,
                granted_permissions: if enabled {
                    vec!["execute".to_owned()]
                } else {
                    Vec::new()
                },
                config: std::collections::BTreeMap::new(),
                updated_at_ms: timestamp,
            }
        })
        .collect::<Vec<_>>();
    bindings.extend(
        crate::agent_harness::default_skill_ids()
            .iter()
            .map(|id| crate::agent_harness::skill_binding_id(id).expect("default skill id"))
            .map(|entry_id| GuruCapabilityBinding {
                guru_id: guru_id.to_owned(),
                entry_id,
                enabled: true,
                granted_permissions: vec!["load".to_owned()],
                config: std::collections::BTreeMap::new(),
                updated_at_ms: timestamp,
            }),
    );
    bindings
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletionKind {
    Chat,
    Guru,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletionPhase {
    Prepared,
    Detached,
}

/// Durable intent for canonical filesystem deletion. SQLite deliberately
/// retains this row after the target row is deleted so startup can distinguish
/// rollback (target exists) from cleanup (target is committed absent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeletionJournal {
    pub id: String,
    pub kind: DeletionKind,
    pub guru_id: String,
    pub target_id: String,
    #[serde(deserialize_with = "required_option")]
    pub expected_source_identity: Option<RootFilesystemIdentity>,
    pub phase: DeletionPhase,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl DeletionJournal {
    pub fn validate(&self) -> Result<(), DomainError> {
        require_identifier(&self.id, "deletion journal id is empty or unsafe")?;
        require_identifier(&self.guru_id, "deletion journal Guru id is empty or unsafe")?;
        require_identifier(
            &self.target_id,
            "deletion journal target id is empty or unsafe",
        )?;
        if self.kind == DeletionKind::Guru && self.target_id != self.guru_id {
            return Err(DomainError::Invalid(
                "Guru deletion journal target is invalid",
            ));
        }
        if (self.kind == DeletionKind::Guru) != self.expected_source_identity.is_some() {
            return Err(DomainError::Invalid(
                "deletion journal source identity is invalid",
            ));
        }
        if self.created_at_ms < 0 || self.updated_at_ms < self.created_at_ms {
            return Err(DomainError::Invalid(
                "deletion journal timestamps are invalid",
            ));
        }
        Ok(())
    }
}
