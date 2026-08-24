use std::collections::BTreeSet;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use super::{validate_canonical_memory_record_id, CanonicalMemoryKind, DomainError};

const MAX_WRITE_TARGETS: usize = 24;
const MAX_EVIDENCE_TARGETS: usize = 3;
const MAX_PROPOSED_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryChangeTarget {
    pub record_id: String,
    pub relative_path: String,
    pub before_markdown: String,
    pub proposed_markdown: String,
}

impl MemoryChangeTarget {
    pub fn validate(&self) -> Result<(), DomainError> {
        self.validate_for_authority(MemoryChangeAuthority::Chat)
    }

    fn validate_for_authority(&self, authority: MemoryChangeAuthority) -> Result<(), DomainError> {
        let kind = self.kind()?;
        validate_canonical_memory_record_id(&self.record_id, Some(kind.slug()))?;
        if self.before_markdown == self.proposed_markdown {
            return Err(DomainError::Invalid("memory target has no change"));
        }
        let deletes_existing = authority == MemoryChangeAuthority::User
            && !self.before_markdown.is_empty()
            && self.proposed_markdown.is_empty();
        if (!deletes_existing && self.proposed_markdown.trim().is_empty())
            || self.before_markdown.contains('\0')
            || self.proposed_markdown.contains('\0')
        {
            return Err(DomainError::Invalid("memory target Markdown is invalid"));
        }
        Ok(())
    }

    pub fn kind(&self) -> Result<CanonicalMemoryKind, DomainError> {
        let path_kind = validate_relative_markdown_path(&self.relative_path)?;
        let id_kind = self
            .record_id
            .split_once(':')
            .map(|(kind, _)| kind)
            .ok_or(DomainError::Invalid("memory record id is invalid"))?;
        if path_kind.slug() != id_kind {
            return Err(DomainError::Invalid(
                "memory target path does not match its record kind",
            ));
        }
        Ok(path_kind)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryChangeAuthority {
    Chat,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryWrite {
    pub guru_id: String,
    pub authority: MemoryChangeAuthority,
    pub targets: Vec<MemoryChangeTarget>,
    pub rationale: String,
}

impl MemoryWrite {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.guru_id.trim().is_empty() {
            return Err(DomainError::Invalid("memory write guru id is empty"));
        }
        if self.rationale.trim().is_empty() {
            return Err(DomainError::Invalid("memory write rationale is empty"));
        }
        if self.targets.is_empty() || self.targets.len() > MAX_WRITE_TARGETS {
            return Err(DomainError::Invalid("memory write target set is invalid"));
        }
        let mut paths = BTreeSet::new();
        let mut records = BTreeSet::new();
        let mut evidence_targets = 0;
        let mut proposed_bytes = 0_usize;
        for target in &self.targets {
            target.validate_for_authority(self.authority)?;
            if target.kind()? == CanonicalMemoryKind::Evidence {
                evidence_targets += 1;
            }
            proposed_bytes = proposed_bytes
                .checked_add(target.proposed_markdown.len())
                .ok_or(DomainError::Invalid(
                    "memory write proposed Markdown is too large",
                ))?;
            if !paths.insert(target.relative_path.as_str()) {
                return Err(DomainError::Invalid("duplicate memory target path"));
            }
            if !records.insert(target.record_id.as_str()) {
                return Err(DomainError::Invalid("duplicate memory record id"));
            }
        }
        if evidence_targets > MAX_EVIDENCE_TARGETS {
            return Err(DomainError::Invalid(
                "change set has too many Evidence targets",
            ));
        }
        if proposed_bytes > MAX_PROPOSED_BYTES {
            return Err(DomainError::Invalid(
                "memory write proposed Markdown is too large",
            ));
        }
        Ok(())
    }
}

fn validate_relative_markdown_path(value: &str) -> Result<CanonicalMemoryKind, DomainError> {
    let path = Path::new(value);
    if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
        return Err(DomainError::Invalid("memory target must be Markdown"));
    }
    if value.contains('\\')
        || path.is_absolute()
        || path.components().any(|component| {
            !matches!(component, Component::Normal(_))
                || component.as_os_str().to_string_lossy().starts_with('.')
        })
    {
        return Err(DomainError::Invalid("memory target path is unsafe"));
    }
    let components = path
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
        .ok_or(DomainError::Invalid("memory target path is unsafe"))?;
    if components.len() < 3 || components[0] != "guruterminal" {
        return Err(DomainError::Invalid(
            "memory target path is outside canonical Memory",
        ));
    }
    CanonicalMemoryKind::from_slug(components[1]).ok_or(DomainError::Invalid(
        "memory target path is outside canonical Memory",
    ))
}
