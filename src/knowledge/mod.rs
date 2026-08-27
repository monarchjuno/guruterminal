//! File-native knowledge records. Markdown is canonical; no index is kept.

mod catalog;
mod context;
mod frontmatter;
mod health;
mod model;
mod query;
mod search;
mod semantic;

use catalog::*;
pub use catalog::{catalog_local, read};
pub use context::context;
use health::*;
pub use health::{check, health};
use query::*;
pub use query::{search_candidates_with_kinds, search_with_kinds_opts};

use frontmatter::parse_frontmatter;
pub use model::*;
use search::{
    combined_relevance, document_relevance, document_sensitive_numbers, explain_match,
    has_minimum_query_coverage, normalize_search_text, search_result, search_result_order,
    section_relevance, section_sensitive_numbers, SearchText, SectionRelevance,
};

use chrono::DateTime;
use guruterminal_core::CanonicalMemoryKind;
use regex::Regex;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
    thread,
};

pub const RELATIONSHIP_TYPES: &[&str] = &["uses", "supports", "contradicts", "updates"];
#[cfg(test)]
mod tests;
