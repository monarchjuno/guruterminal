use super::{split_sections, Document, SearchText};
use model2vec_rs::model::StaticModel;
use std::sync::OnceLock;
use tokenizers as _;

const SEMANTIC_THRESHOLD: f32 = 0.38;
const BODY_EXCERPT_CHARS: usize = 280;

fn static_model() -> &'static StaticModel {
    static MODEL: OnceLock<StaticModel> = OnceLock::new();
    MODEL.get_or_init(|| {
        StaticModel::from_bytes(
            include_bytes!("embed_assets/tokenizer.json"),
            include_bytes!("embed_assets/model.safetensors"),
            include_bytes!("embed_assets/config.json"),
            Some(true),
        )
        .expect("bundled Model2Vec embedding model must load")
    })
}

pub(super) fn embed_query(query: &SearchText) -> Option<Vec<f32>> {
    if !query.semantic_eligible() {
        return None;
    }
    Some(static_model().encode_single(query.normalized()))
}

pub(super) fn score_document(doc: &Document, query_embedding: &[f32]) -> Option<i32> {
    let text = document_embedding_text(doc);
    if !text.chars().any(|character| character.is_alphanumeric()) {
        return None;
    }
    let cosine = cosine(query_embedding, &static_model().encode_single(&text));
    if cosine < SEMANTIC_THRESHOLD {
        return None;
    }
    Some(semantic_score(cosine))
}

fn semantic_score(cosine: f32) -> i32 {
    let span = (1.0 - SEMANTIC_THRESHOLD).max(f32::EPSILON);
    let scaled = ((cosine - SEMANTIC_THRESHOLD) / span * 900.0).round() as i32;
    1_000 + scaled.clamp(0, 900)
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut left_norm = 0.0f32;
    let mut right_norm = 0.0f32;
    for (a, b) in left.iter().zip(right.iter()) {
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot / (left_norm.sqrt() * right_norm.sqrt())
    }
}

fn document_embedding_text(doc: &Document) -> String {
    let mut parts = Vec::new();
    for value in [doc.title.as_str(), doc.summary.as_str()] {
        if !value.is_empty() {
            parts.push(value.to_owned());
        }
    }
    parts.extend(doc.aliases.iter().cloned());
    parts.extend(doc.entities.iter().cloned());
    parts.extend(doc.tags.iter().cloned());
    if let Some(period) = &doc.period {
        parts.push(period.clone());
    }
    if let Some(section) = split_sections(&doc.body).first() {
        if !section.heading.is_empty() {
            parts.push(section.heading.clone());
        }
        if let Some(excerpt) = first_paragraph_excerpt(&section.text, BODY_EXCERPT_CHARS) {
            parts.push(excerpt);
        }
    }
    parts.join(" ")
}

fn first_paragraph_excerpt(text: &str, max_chars: usize) -> Option<String> {
    let paragraph = text
        .split("\n\n")
        .map(str::trim)
        .find(|part| part.chars().any(|character| character.is_alphanumeric()))?;
    let excerpt: String = paragraph.chars().take(max_chars).collect();
    excerpt
        .chars()
        .any(|character| character.is_alphanumeric())
        .then_some(excerpt)
}
