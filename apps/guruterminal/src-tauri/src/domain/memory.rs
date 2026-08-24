use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    hex_lower, require_bounded_text, require_identifier, required_option, sha256_hex,
    validate_canonical_memory_record_id, CanonicalMemoryKind, DomainError,
};

const MAX_MEMORY_PROPOSAL_BYTES: usize = 128 * 1024;

pub(crate) const MAX_MEMORY_REFS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAccess {
    SearchDiscovered,
    ExactRead,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryRefSnapshot {
    pub record_id: String,
    pub kind: String,
    pub title: String,
    pub excerpt: String,
    #[serde(deserialize_with = "required_option")]
    pub as_of: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub section: Option<String>,
    pub access: MemoryAccess,
    #[serde(deserialize_with = "required_option")]
    pub full_record_digest: Option<String>,
}

impl MemoryRefSnapshot {
    pub fn validate(&self) -> Result<(), DomainError> {
        let kind = CanonicalMemoryKind::from_label(&self.kind)
            .ok_or(DomainError::Invalid("memory reference kind is invalid"))?;
        validate_canonical_memory_record_id(&self.record_id, Some(kind.slug()))?;
        require_bounded_text(&self.title, 512, "memory reference title is invalid")?;
        if self.excerpt.len() > 2_048 || self.excerpt.contains('\0') {
            return Err(DomainError::Invalid("memory reference excerpt is invalid"));
        }
        if self
            .as_of
            .as_deref()
            .is_some_and(|value| value.is_empty() || value.len() > 128 || value.contains('\0'))
        {
            return Err(DomainError::Invalid("memory reference as-of is invalid"));
        }
        if self.section.as_deref().is_some_and(|value| {
            value.is_empty() || value.len() > 512 || value.contains(['\0', '\n', '\r'])
        }) {
            return Err(DomainError::Invalid("memory reference section is invalid"));
        }
        let is_full_record_read = self.access == MemoryAccess::ExactRead && self.section.is_none();
        if self.full_record_digest.is_some() != is_full_record_read
            || self
                .full_record_digest
                .as_deref()
                .is_some_and(|digest| !is_sha256_digest(digest))
        {
            return Err(DomainError::Invalid(
                "memory reference full-record digest is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
pub enum MemoryProposalBase {
    Absent,
    FullRead { digest: String },
}

impl MemoryProposalBase {
    fn validate(&self) -> Result<(), DomainError> {
        if let Self::FullRead { digest } = self {
            if !is_sha256_digest(digest) {
                return Err(DomainError::Invalid(
                    "memory proposal base digest is invalid",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryProposal {
    pub id: String,
    pub target_kind: String,
    pub target_record_id: String,
    pub target_base: MemoryProposalBase,
    pub proposed_markdown: String,
    pub rationale: String,
    pub digest: String,
    pub source_memory_ids: Vec<String>,
    #[serde(deserialize_with = "required_option")]
    pub source_message_id: Option<String>,
}

impl MemoryProposal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        target_kind: String,
        target_record_id: String,
        target_base: MemoryProposalBase,
        proposed_markdown: String,
        rationale: String,
        source_memory_ids: Vec<String>,
        source_message_id: Option<String>,
    ) -> Result<Self, DomainError> {
        let digest = sha256_hex(&[proposed_markdown.as_bytes()]);
        let proposal = Self {
            id,
            target_kind,
            target_record_id,
            target_base,
            proposed_markdown,
            rationale,
            digest,
            source_memory_ids,
            source_message_id,
        };
        proposal.validate()?;
        Ok(proposal)
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        require_identifier(&self.id, "memory proposal id is empty or unsafe")?;
        let kind = CanonicalMemoryKind::from_slug(&self.target_kind.to_ascii_lowercase())
            .filter(|kind| matches!(kind, CanonicalMemoryKind::Wiki | CanonicalMemoryKind::Lens))
            .ok_or(DomainError::Invalid("memory proposal kind is invalid"))?;
        validate_canonical_memory_record_id(&self.target_record_id, Some(kind.slug()))?;
        self.target_base.validate()?;
        if self.proposed_markdown.trim().is_empty()
            || self.proposed_markdown.len() > MAX_MEMORY_PROPOSAL_BYTES
            || self.proposed_markdown.contains('\0')
        {
            return Err(DomainError::Invalid("memory proposal Markdown is invalid"));
        }
        let declared_id = markdown_frontmatter_id(&self.proposed_markdown).ok_or(
            DomainError::Invalid("memory proposal Markdown id is missing"),
        )?;
        if declared_id != self.target_record_id {
            return Err(DomainError::Invalid(
                "memory proposal Markdown id must equal the target id",
            ));
        }
        for (field, message) in [
            ("title", "memory proposal Markdown title is required"),
            ("summary", "memory proposal Markdown summary is required"),
            ("as_of", "memory proposal Markdown as_of is required"),
        ] {
            if markdown_frontmatter_scalar(&self.proposed_markdown, field).is_none() {
                return Err(DomainError::Invalid(message));
            }
        }
        if markdown_frontmatter_scalar(&self.proposed_markdown, "as_of")
            .is_some_and(|as_of| !markdown_as_of_is_rfc3339_seconds(&as_of))
        {
            return Err(DomainError::Invalid(
                "memory proposal Markdown as_of must be RFC3339 with seconds and timezone",
            ));
        }
        if matches!(kind, CanonicalMemoryKind::Lens)
            && !lens_proposal_has_required_sections(&self.proposed_markdown)
        {
            return Err(DomainError::Invalid(
                "Lens proposals require Scope, Assumptions, Counterexamples, Limits, and Invalidation conditions",
            ));
        }
        require_bounded_text(
            &self.rationale,
            8_192,
            "memory proposal rationale is invalid",
        )?;
        if self.digest != sha256_hex(&[self.proposed_markdown.as_bytes()]) {
            return Err(DomainError::Invalid("memory proposal digest is invalid"));
        }
        if self.source_memory_ids.len() > MAX_MEMORY_REFS {
            return Err(DomainError::Invalid(
                "memory proposal has too many source records",
            ));
        }
        let mut unique = BTreeSet::new();
        for source in &self.source_memory_ids {
            if validate_canonical_memory_record_id(source, None).is_err() {
                return Err(DomainError::Invalid("memory proposal source id is invalid"));
            }
            if !unique.insert(source.as_str()) {
                return Err(DomainError::Invalid(
                    "memory proposal source ids contain duplicates",
                ));
            }
        }
        if self.source_message_id.as_deref().is_some_and(|value| {
            value.is_empty() || value.len() > 512 || value.contains(['\0', '\n', '\r'])
        }) {
            return Err(DomainError::Invalid(
                "memory proposal source message is invalid",
            ));
        }
        Ok(())
    }
}

fn is_sha256_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

const LENS_REQUIRED_HEADINGS: &[&str] = &[
    "scope",
    "assumptions",
    "counterexamples",
    "limits",
    "invalidation conditions",
];

fn markdown_as_of_is_rfc3339_seconds(value: &str) -> bool {
    static RFC3339_SECONDS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RFC3339_SECONDS
        .get_or_init(|| {
            regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:Z|[+-]\d{2}:\d{2})$")
                .expect("RFC 3339 seconds regex is valid")
        })
        .is_match(value)
        && chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

pub(crate) fn markdown_frontmatter_id(markdown: &str) -> Option<String> {
    markdown_frontmatter_scalar(markdown, "id")
}

pub(crate) fn markdown_frontmatter_scalar(markdown: &str, key: &str) -> Option<String> {
    for (line_key, value, _) in markdown_frontmatter_entries(markdown) {
        if line_key == key && !value.is_empty() {
            return Some(value);
        }
    }
    None
}

pub(crate) fn markdown_frontmatter_list(markdown: &str, key: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut unique = BTreeSet::new();
    let mut collecting = false;
    for (line_key, value, raw) in markdown_frontmatter_entries(markdown) {
        if collecting {
            let trimmed = raw.trim();
            if let Some(item) = trimmed.strip_prefix("- ") {
                let item = unquote_frontmatter_value(item.trim());
                if !item.is_empty() && unique.insert(item.clone()) {
                    values.push(item);
                }
                continue;
            }
            collecting = false;
        }
        if line_key != key {
            continue;
        }
        if value.is_empty() {
            collecting = true;
            continue;
        }
        let inner = value
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .unwrap_or(value.as_str());
        for item in inner.split(',') {
            let item = unquote_frontmatter_value(item.trim());
            if !item.is_empty() && unique.insert(item.clone()) {
                values.push(item);
            }
        }
    }
    values
}

pub(crate) fn normalize_memory_label(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

pub(crate) fn memory_identity_labels(title: &str, aliases: &[String]) -> BTreeSet<String> {
    std::iter::once(title)
        .chain(aliases.iter().map(String::as_str))
        .map(normalize_memory_label)
        .filter(|value| !value.is_empty())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemoryIdentityRecord {
    pub id: String,
    pub title: String,
    pub aliases: Vec<String>,
    pub status: Option<String>,
}

pub(crate) fn colliding_active_memory_id(
    target_id: &str,
    proposed_title: &str,
    proposed_aliases: &[String],
    records: &[MemoryIdentityRecord],
) -> Option<String> {
    let proposed = memory_identity_labels(proposed_title, proposed_aliases);
    if proposed.is_empty() {
        return None;
    }
    records.iter().find_map(|record| {
        if record.id == target_id || record.status.as_deref() == Some("revoked") {
            return None;
        }
        let existing = memory_identity_labels(&record.title, &record.aliases);
        existing
            .intersection(&proposed)
            .next()
            .map(|_| record.id.clone())
    })
}

fn markdown_frontmatter_entries(markdown: &str) -> Vec<(String, String, String)> {
    let mut lines = markdown.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Vec::new();
    }
    let mut entries = Vec::new();
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let raw = line.to_owned();
        if let Some((key, value)) = line.split_once(':') {
            entries.push((
                key.trim().to_owned(),
                unquote_frontmatter_value(value.trim()),
                raw,
            ));
        } else {
            entries.push((String::new(), String::new(), raw));
        }
    }
    entries
}

fn unquote_frontmatter_value(value: &str) -> String {
    let cleaned = if value.len() >= 2
        && ((value.starts_with('\'') && value.ends_with('\''))
            || (value.starts_with('"') && value.ends_with('"')))
    {
        value[1..value.len() - 1].trim()
    } else {
        value
    };
    cleaned.to_owned()
}

pub(crate) fn lens_proposal_has_required_sections(markdown: &str) -> bool {
    let body = markdown_body_after_frontmatter(markdown);
    let mut sections = std::collections::BTreeMap::<String, String>::new();
    let mut current = None::<String>;
    let mut buffer = String::new();
    let flush = |sections: &mut std::collections::BTreeMap<String, String>,
                 current: &Option<String>,
                 buffer: &mut String| {
        if let Some(name) = current {
            sections.insert(name.clone(), std::mem::take(buffer));
        } else {
            buffer.clear();
        }
    };
    for line in body.lines() {
        let hashes = line
            .chars()
            .take_while(|character| *character == '#')
            .count();
        if hashes > 0 && line.as_bytes().get(hashes) == Some(&b' ') {
            flush(&mut sections, &current, &mut buffer);
            current = Some(line[hashes + 1..].trim().to_ascii_lowercase());
            continue;
        }
        buffer.push_str(line);
        buffer.push('\n');
    }
    flush(&mut sections, &current, &mut buffer);
    LENS_REQUIRED_HEADINGS.iter().all(|heading| {
        sections
            .get(*heading)
            .is_some_and(|value| !value.trim().is_empty())
    })
}

fn markdown_body_after_frontmatter(markdown: &str) -> &str {
    let mut lines = markdown.lines();
    if lines.next().map(str::trim) != Some("---") {
        return markdown;
    }
    markdown
        .splitn(3, "---")
        .nth(2)
        .map(|body| body.strip_prefix('\n').unwrap_or(body))
        .unwrap_or("")
}

pub(crate) fn memory_refs_digest(memories: &[MemoryRefSnapshot]) -> Result<String, DomainError> {
    let mut canonical = memories.to_vec();
    canonical.sort_by(|left, right| left.record_id.cmp(&right.record_id));
    let encoded = serde_json::to_vec(&canonical)
        .map_err(|_| DomainError::Invalid("chat memory references cannot be encoded"))?;
    Ok(hex_lower(&Sha256::digest(encoded)))
}
