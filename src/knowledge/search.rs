use super::{Document, MatchTier, SearchResult, Section};
use chrono::DateTime;
use regex::Regex;
use std::{
    collections::{BTreeSet, HashSet},
    sync::OnceLock,
};

#[derive(Clone, Debug)]
pub(super) struct SearchText {
    normalized: String,
    compact: String,
    compact_eligible: bool,
    pub(super) sensitive_numbers: BTreeSet<String>,
    pub(super) tokens: BTreeSet<String>,
}

impl SearchText {
    pub(super) fn new(value: &str) -> Self {
        let normalized = normalize_search_text(value);
        let compact_eligible = normalized.chars().any(is_spacing_insensitive_script)
            && normalized.chars().all(|character| {
                !character.is_alphanumeric() || is_spacing_insensitive_script(character)
            });
        let compact = normalized
            .chars()
            .filter(|character| character.is_alphanumeric())
            .collect();
        let sensitive_numbers = sensitive_numbers(&normalized);
        let tokens = search_tokens(&normalized);
        Self {
            normalized,
            compact,
            compact_eligible,
            sensitive_numbers,
            tokens,
        }
    }

    pub(super) fn normalized(&self) -> &str {
        &self.normalized
    }

    pub(super) fn semantic_eligible(&self) -> bool {
        !self.compact_eligible
            && self.sensitive_numbers.is_empty()
            && !self.normalized.trim().is_empty()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Relevance {
    pub(super) tier: i32,
    points: i32,
}

impl Relevance {
    fn add(&mut self, other: Self) {
        self.tier = self.tier.max(other.tier);
        self.points = self.points.saturating_add(other.points);
    }

    fn combined(mut self, other: Self) -> Self {
        self.add(other);
        self
    }

    pub(super) fn is_match(self) -> bool {
        self.tier > 0
    }

    pub(super) fn score(self) -> i32 {
        self.tier * 1_000 + self.points.min(999)
    }
}

#[derive(Clone, Debug, Default)]
struct ScoredField {
    relevance: Relevance,
    matched_terms: BTreeSet<String>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct DocumentRelevance {
    pub(super) relevance: Relevance,
    pub(super) concise_terms: BTreeSet<String>,
    authored_anchor: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct SectionRelevance {
    pub(super) relevance: Relevance,
    concise_terms: BTreeSet<String>,
    all_terms: BTreeSet<String>,
}

pub(super) fn normalize_search_text(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\u{2212}' | '\u{fe63}' | '\u{ff0d}' => '-',
            '\u{fe62}' | '\u{ff0b}' => '+',
            '\u{ff05}' => '%',
            _ => character,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

fn identifier_pattern() -> &'static Regex {
    static IDENTIFIERS: OnceLock<Regex> = OnceLock::new();
    IDENTIFIERS.get_or_init(|| Regex::new(r"[\w:./+@#-]+").unwrap())
}

fn component_pattern() -> &'static Regex {
    static COMPONENTS: OnceLock<Regex> = OnceLock::new();
    COMPONENTS.get_or_init(|| Regex::new(r"\w+").unwrap())
}

fn sensitive_number_pattern() -> &'static Regex {
    static SENSITIVE_NUMBERS: OnceLock<Regex> = OnceLock::new();
    SENSITIVE_NUMBERS.get_or_init(|| {
        Regex::new(r"(?:^|[^\w])([+-]?\d+(?:\.\d+)?(?:/\d+(?:\.\d+)?)?%?)").unwrap()
    })
}

fn is_spacing_insensitive_script(character: char) -> bool {
    let codepoint = character as u32;
    (0x1100..=0x11ff).contains(&codepoint)
        || (0x3040..=0x30ff).contains(&codepoint)
        || (0x3130..=0x318f).contains(&codepoint)
        || (0x31f0..=0x31ff).contains(&codepoint)
        || (0x3400..=0x4dbf).contains(&codepoint)
        || (0x4e00..=0x9fff).contains(&codepoint)
        || (0xa960..=0xa97f).contains(&codepoint)
        || (0xac00..=0xd7af).contains(&codepoint)
        || (0xd7b0..=0xd7ff).contains(&codepoint)
        || (0xf900..=0xfaff).contains(&codepoint)
        || (0xff65..=0xff9f).contains(&codepoint)
        || (0x20000..=0x323af).contains(&codepoint)
}

fn sensitive_numbers(normalized: &str) -> BTreeSet<String> {
    sensitive_number_pattern()
        .captures_iter(normalized)
        .filter_map(|captures| captures.get(1))
        .map(|value| value.as_str())
        .filter(|value| {
            value
                .chars()
                .any(|character| matches!(character, '+' | '-' | '.' | '/' | '%'))
        })
        .map(str::to_owned)
        .collect()
}

fn contains_bounded_phrase(value: &str, query: &str) -> bool {
    let Some(first) = query.chars().next() else {
        return false;
    };
    let last = query
        .chars()
        .next_back()
        .expect("a query with a first character has a last character");
    value.match_indices(query).any(|(start, matched)| {
        let end = start + matched.len();
        let starts_at_boundary = !first.is_alphanumeric()
            || value[..start]
                .chars()
                .next_back()
                .is_none_or(|character| !character.is_alphanumeric());
        let ends_at_boundary = !last.is_alphanumeric()
            || value[end..]
                .chars()
                .next()
                .is_none_or(|character| !character.is_alphanumeric());
        starts_at_boundary && ends_at_boundary
    })
}

fn english_stopwords() -> &'static HashSet<&'static str> {
    static STOPWORDS: OnceLock<HashSet<&'static str>> = OnceLock::new();
    STOPWORDS.get_or_init(|| {
        HashSet::from([
            "a", "an", "the", "and", "or", "but", "if", "then", "than", "so", "of", "in", "on",
            "at", "to", "for", "from", "by", "with", "as", "into", "about", "over", "after",
            "before", "is", "are", "was", "were", "be", "been", "being", "am", "it", "its", "this",
            "that", "these", "those", "i", "we", "you", "they", "he", "she", "them", "my", "our",
            "your", "what", "which", "who", "whom", "whose", "when", "where", "why", "how",
            "should", "would", "could", "can", "will", "may", "might", "must", "do", "does", "did",
            "doing", "not", "no", "nor", "just", "also", "very", "more", "most", "such",
        ])
    })
}

fn english_token_variants(token: &str) -> impl Iterator<Item = String> {
    let mut variants = vec![token.to_owned()];
    if token.chars().count() < 5 || english_stopwords().contains(token) {
        return variants.into_iter();
    }
    let push_stem = |variants: &mut Vec<String>, stem: &str| {
        if stem.chars().count() >= 4 && !english_stopwords().contains(stem) {
            variants.push(stem.to_owned());
        }
    };
    if let Some(stem) = token.strip_suffix("ing") {
        push_stem(&mut variants, stem);
        let with_e = format!("{stem}e");
        push_stem(&mut variants, &with_e);
    }
    if let Some(stem) = token.strip_suffix("ies") {
        let singular = format!("{stem}y");
        push_stem(&mut variants, &singular);
    }
    if let Some(stem) = token.strip_suffix("es") {
        push_stem(&mut variants, stem);
    }
    if !token.ends_with("ss") {
        if let Some(stem) = token.strip_suffix('s') {
            push_stem(&mut variants, stem);
        }
    }
    if let Some(stem) = token.strip_suffix("ed") {
        push_stem(&mut variants, stem);
        let with_e = format!("{stem}e");
        push_stem(&mut variants, &with_e);
    }
    variants.into_iter()
}

fn search_tokens(normalized: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for identifier in identifier_pattern().find_iter(normalized) {
        let value = identifier.as_str();
        if !english_stopwords().contains(value) {
            out.insert(value.to_owned());
        }
        for component in component_pattern().find_iter(value) {
            let component = component.as_str();
            if english_stopwords().contains(component) {
                continue;
            }
            out.extend(english_token_variants(component));
        }
    }
    out
}

fn score_field(value: &str, weight: i32, query: &SearchText, allow_compact: bool) -> ScoredField {
    if value.is_empty() {
        return ScoredField::default();
    }
    let value = SearchText::new(value);
    let compact_match =
        allow_compact && query.compact_eligible && query.compact.chars().count() >= 3;
    let exact =
        value.normalized == query.normalized || (compact_match && value.compact == query.compact);
    let phrase = (query.tokens.len() > 1
        && contains_bounded_phrase(&value.normalized, &query.normalized))
        || (compact_match && value.compact.contains(&query.compact));
    let mut matched_terms = value
        .tokens
        .intersection(&query.tokens)
        .cloned()
        .collect::<BTreeSet<_>>();
    let matched = matched_terms.len() as i32;
    let query_terms = query.tokens.len() as i32;
    let tier = if exact {
        4
    } else if phrase {
        3
    } else if query_terms > 0 && matched == query_terms {
        2
    } else if matched > 0 {
        1
    } else {
        0
    };
    if compact_match && value.compact.contains(&query.compact) && matched_terms.is_empty() {
        matched_terms.extend(query.tokens.iter().cloned());
    }
    ScoredField {
        relevance: Relevance {
            tier,
            points: if tier > 0 { matched.max(1) * weight } else { 0 },
        },
        matched_terms,
    }
}

fn add_document_field(
    result: &mut DocumentRelevance,
    value: &str,
    weight: i32,
    query: &SearchText,
    allow_compact: bool,
) {
    let field = score_field(value, weight, query, allow_compact);
    result.relevance.add(field.relevance);
    result.concise_terms.extend(field.matched_terms);
}

fn contains_authored_anchor(value: &str, query: &SearchText) -> bool {
    let value = normalize_search_text(value);
    !value.is_empty()
        && value != query.normalized
        && contains_bounded_phrase(&query.normalized, &value)
}

pub(super) fn document_relevance(doc: &Document, query: &SearchText) -> DocumentRelevance {
    let id = score_field(&doc.id, 14, query, false);
    let authored_anchor = contains_authored_anchor(&doc.id, query)
        || doc
            .aliases
            .iter()
            .any(|value| contains_authored_anchor(value, query))
        || doc
            .entities
            .iter()
            .any(|value| contains_authored_anchor(value, query));
    let mut result = DocumentRelevance {
        relevance: id.relevance,
        concise_terms: id.matched_terms,
        authored_anchor,
    };
    if normalize_search_text(&doc.id) == query.normalized {
        result.relevance.tier = 5;
    }
    add_document_field(&mut result, &doc.title, 12, query, true);
    for value in &doc.aliases {
        add_document_field(&mut result, value, 12, query, true);
    }
    for value in &doc.entities {
        add_document_field(&mut result, value, 12, query, false);
    }
    for value in &doc.tags {
        add_document_field(&mut result, value, 8, query, true);
    }
    add_document_field(
        &mut result,
        doc.period.as_deref().unwrap_or_default(),
        8,
        query,
        false,
    );
    add_document_field(&mut result, &doc.summary, 7, query, false);
    for relationship in &doc.relationships {
        add_document_field(&mut result, &relationship.target, 4, query, false);
    }
    if result.authored_anchor {
        result.relevance.tier = result.relevance.tier.max(3);
    }
    if !query.tokens.is_empty() && query.tokens.is_subset(&result.concise_terms) {
        result.relevance.tier = result.relevance.tier.max(2);
        result.relevance.points = result
            .relevance
            .points
            .saturating_add(query.tokens.len() as i32 * 6);
    }
    result
}

pub(super) fn section_relevance(section: &Section, query: &SearchText) -> SectionRelevance {
    let heading = score_field(&section.heading, 8, query, true);
    let mut body = score_field(&section.text, 4, query, false);
    if body.relevance.tier == 4 {
        body.relevance.tier = if query.tokens.len() > 1 { 3 } else { 1 };
    } else if body.relevance.tier == 2 {
        body.relevance.tier = 1;
    }
    let mut all_terms = heading.matched_terms.clone();
    all_terms.extend(body.matched_terms);
    SectionRelevance {
        relevance: heading.relevance.combined(body.relevance),
        concise_terms: heading.matched_terms,
        all_terms,
    }
}

pub(super) fn combined_relevance(
    document: &DocumentRelevance,
    section: &SectionRelevance,
    query: &SearchText,
) -> Relevance {
    let mut relevance = document.relevance.combined(section.relevance);
    if !query.tokens.is_empty()
        && query.tokens.iter().all(|term| {
            document.concise_terms.contains(term) || section.concise_terms.contains(term)
        })
    {
        relevance.tier = relevance.tier.max(2);
        relevance.points = relevance
            .points
            .saturating_add(query.tokens.len() as i32 * 4);
    }
    relevance
}

pub(super) fn document_sensitive_numbers(doc: &Document) -> BTreeSet<String> {
    let mut numbers = BTreeSet::new();
    for value in [
        doc.id.as_str(),
        doc.title.as_str(),
        doc.summary.as_str(),
        doc.period.as_deref().unwrap_or_default(),
    ]
    .into_iter()
    .chain(doc.aliases.iter().map(String::as_str))
    .chain(doc.entities.iter().map(String::as_str))
    .chain(doc.tags.iter().map(String::as_str))
    .chain(
        doc.relationships
            .iter()
            .map(|relationship| relationship.target.as_str()),
    ) {
        numbers.extend(sensitive_numbers(&normalize_search_text(value)));
    }
    numbers
}

pub(super) fn section_sensitive_numbers(section: &Section) -> BTreeSet<String> {
    let mut numbers = sensitive_numbers(&normalize_search_text(&section.heading));
    numbers.extend(sensitive_numbers(&normalize_search_text(&section.text)));
    numbers
}

pub(super) fn search_result(doc: &Document, section: &Section, score: i32) -> SearchResult {
    SearchResult {
        id: doc.id.clone(),
        kind: doc.kind.clone(),
        title: doc.title.clone(),
        summary: doc.summary.clone(),
        as_of: doc.as_of.clone(),
        path: doc.path.clone(),
        section: section.heading.clone(),
        heading_path: section.heading_path.clone(),
        entities: doc.entities.clone(),
        period: doc.period.clone(),
        relationships: doc.relationships.clone(),
        score,
        text: contextual(doc, section),
        status: doc.status.clone(),
        aliases: doc.aliases.clone(),
        tags: doc.tags.clone(),
        match_tier: MatchTier::Partial,
        matched_fields: Vec::new(),
        matched_terms: Vec::new(),
    }
}

pub(super) fn has_minimum_query_coverage(
    relevance: Relevance,
    document: &DocumentRelevance,
    section: &SectionRelevance,
    query: &SearchText,
) -> bool {
    query.tokens.len() <= 1
        || document.authored_anchor
        || relevance.tier >= 2
        || query
            .tokens
            .iter()
            .filter(|term| {
                document.concise_terms.contains(*term) || section.all_terms.contains(*term)
            })
            .take(2)
            .count()
            >= 2
}

pub(super) fn explain_match(
    doc: &Document,
    section: &Section,
    query: &SearchText,
) -> (MatchTier, Vec<String>, Vec<String>) {
    let mut fields = BTreeSet::new();
    let mut terms = BTreeSet::new();
    let mut exact_metadata = false;
    let mut phrase = false;
    let named_fields = [
        ("id", doc.id.as_str(), false),
        ("title", doc.title.as_str(), true),
        ("summary", doc.summary.as_str(), false),
        ("period", doc.period.as_deref().unwrap_or_default(), false),
    ];
    for (name, value, compact) in named_fields {
        collect_match_details(
            name,
            value,
            compact,
            query,
            &mut fields,
            &mut terms,
            &mut exact_metadata,
            &mut phrase,
        );
    }
    for (name, values, compact) in [
        ("aliases", &doc.aliases, true),
        ("entities", &doc.entities, false),
        ("tags", &doc.tags, true),
    ] {
        for value in values {
            collect_match_details(
                name,
                value,
                compact,
                query,
                &mut fields,
                &mut terms,
                &mut exact_metadata,
                &mut phrase,
            );
        }
    }
    for relationship in &doc.relationships {
        collect_match_details(
            "relationships",
            &relationship.target,
            false,
            query,
            &mut fields,
            &mut terms,
            &mut exact_metadata,
            &mut phrase,
        );
    }
    collect_match_details(
        "heading",
        &section.heading,
        true,
        query,
        &mut fields,
        &mut terms,
        &mut exact_metadata,
        &mut phrase,
    );
    let concise_terms = terms.clone();
    let mut body = score_field(&section.text, 4, query, false);
    if body.relevance.tier == 4 {
        body.relevance.tier = if query.tokens.len() > 1 { 3 } else { 1 };
    } else if body.relevance.tier == 2 {
        body.relevance.tier = 1;
    }
    if body.relevance.is_match() {
        fields.insert("body".to_owned());
        terms.extend(body.matched_terms);
        phrase |= body.relevance.tier >= 3;
    }

    let match_tier = if normalize_search_text(&doc.id) == query.normalized {
        MatchTier::ExactId
    } else if exact_metadata {
        MatchTier::ExactMetadata
    } else if phrase {
        MatchTier::Phrase
    } else if !query.tokens.is_empty() && query.tokens.is_subset(&concise_terms) {
        MatchTier::AllTerms
    } else {
        MatchTier::Partial
    };
    (
        match_tier,
        fields.into_iter().collect(),
        terms.into_iter().collect(),
    )
}

#[allow(clippy::too_many_arguments)]
fn collect_match_details(
    name: &str,
    value: &str,
    allow_compact: bool,
    query: &SearchText,
    fields: &mut BTreeSet<String>,
    terms: &mut BTreeSet<String>,
    exact_metadata: &mut bool,
    phrase: &mut bool,
) {
    let field = score_field(value, 1, query, allow_compact);
    if field.relevance.is_match() {
        fields.insert(name.to_owned());
        terms.extend(field.matched_terms);
        *exact_metadata |= field.relevance.tier >= 4;
        *phrase |= field.relevance.tier == 3;
    }
}
pub(super) fn search_result_order(a: &SearchResult, b: &SearchResult) -> std::cmp::Ordering {
    b.score
        .cmp(&a.score)
        .then_with(|| search_timestamp(&b.as_of).cmp(&search_timestamp(&a.as_of)))
        .then_with(|| a.id.cmp(&b.id))
        .then_with(|| a.heading_path.cmp(&b.heading_path))
}
fn search_timestamp(value: &str) -> i64 {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.timestamp())
        .unwrap_or(i64::MIN)
}
fn contextual(doc: &Document, section: &Section) -> String {
    let entities = doc.entities.join(" ");
    let period = doc.period.clone().unwrap_or_default();
    let mut header = Vec::new();
    for value in [
        doc.title.as_str(),
        doc.summary.as_str(),
        entities.as_str(),
        period.as_str(),
        doc.as_of.as_str(),
        section.heading.as_str(),
    ] {
        if !value.is_empty() {
            header.push(value);
        }
    }
    header.join("\n")
}
