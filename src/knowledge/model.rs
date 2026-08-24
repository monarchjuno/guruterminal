use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Document {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub as_of: String,
    pub path: String,
    pub entities: Vec<String>,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub see_also: Vec<String>,
    pub source: Option<String>,
    pub period: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_by: Option<String>,
    pub relationships: Vec<Relationship>,
    #[serde(skip_serializing)]
    pub content: String,
    #[serde(skip_serializing)]
    pub body: String,
    #[serde(skip_serializing)]
    pub read_error: Option<String>,
    #[serde(skip_serializing)]
    pub declared_fields: BTreeSet<String>,
    #[serde(skip_serializing)]
    pub duplicate_fields: BTreeSet<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq, Ord, PartialOrd)]
pub struct Relationship {
    #[serde(rename = "type")]
    pub kind: String,
    pub target: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Section {
    pub heading: String,
    pub heading_path: Vec<String>,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SearchResult {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub as_of: String,
    pub path: String,
    pub section: String,
    pub heading_path: Vec<String>,
    pub entities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    pub relationships: Vec<Relationship>,
    pub score: i32,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing)]
    pub aliases: Vec<String>,
    #[serde(skip_serializing)]
    pub tags: Vec<String>,
    #[serde(skip_serializing)]
    pub match_tier: MatchTier,
    #[serde(skip_serializing)]
    pub matched_fields: Vec<String>,
    #[serde(skip_serializing)]
    pub matched_terms: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatchTier {
    ExactId,
    ExactMetadata,
    Phrase,
    AllTerms,
    #[default]
    Partial,
}

impl MatchTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ExactId => "exact_id",
            Self::ExactMetadata => "exact_metadata",
            Self::Phrase => "phrase",
            Self::AllTerms => "all_terms",
            Self::Partial => "partial",
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CandidateResult {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub as_of: String,
    pub section: String,
    pub heading_path: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub score: i32,
    pub match_tier: MatchTier,
    pub matched_fields: Vec<String>,
    pub matched_terms: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct KnowledgeIssue {
    pub path: String,
    pub field: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct KnowledgeCheck {
    pub valid: bool,
    pub documents: usize,
    pub errors: Vec<KnowledgeIssue>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq, Ord, PartialOrd)]
pub struct HealthAdvisory {
    pub code: String,
    pub ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct KindHealth {
    pub kind: String,
    pub documents: usize,
    pub folders: usize,
    pub max_depth: usize,
    pub review_band: String,
    pub advisories: Vec<HealthAdvisory>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct KnowledgeHealth {
    pub kinds: Vec<KindHealth>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ReadResult {
    pub document: Document,
    pub section: Option<Section>,
}
