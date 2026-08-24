use super::{semantic, *};
use chrono::NaiveDate;

pub(super) const PARALLEL_SCAN_MIN_FILES: usize = 256;
pub(super) const MAX_SCAN_WORKERS: usize = 8;

#[derive(Clone, Debug)]
pub(super) struct RankedSearchResult {
    result: SearchResult,
    document: Document,
    section: Section,
}

#[cfg(test)]
pub fn search_with_kinds(
    root: &Path,
    query: &str,
    kinds: &[String],
    limit: usize,
) -> Result<Vec<SearchResult>, String> {
    search_with_kinds_opts(root, query, kinds, limit, false, None)
}

pub fn search_with_kinds_opts(
    root: &Path,
    query: &str,
    kinds: &[String],
    limit: usize,
    include_revoked: bool,
    as_of: Option<NaiveDate>,
) -> Result<Vec<SearchResult>, String> {
    search_internal(root, query, kinds, limit, false, include_revoked, as_of)
}

pub fn search_candidates_with_kinds(
    root: &Path,
    query: &str,
    kinds: &[String],
    limit: usize,
) -> Result<Vec<CandidateResult>, String> {
    search_internal(root, query, kinds, limit, true, false, None)
        .map(|results| candidate_results(&results))
}

pub(super) fn search_internal(
    root: &Path,
    query: &str,
    kinds: &[String],
    limit: usize,
    explain: bool,
    include_revoked: bool,
    as_of: Option<NaiveDate>,
) -> Result<Vec<SearchResult>, String> {
    if query.trim().is_empty() {
        return Err("knowledge search query must not be empty".into());
    }
    for k in kinds {
        if CanonicalMemoryKind::from_slug(k).is_none() {
            return Err(format!("unknown knowledge kind: {k}"));
        }
    }
    if !(1..=50).contains(&limit) {
        return Err("knowledge search limit must be between 1 and 50".into());
    }
    let paths = local_markdown_files_for_kinds(root, kinds);
    let query = SearchText::new(query.trim());
    let out = search_paths(root, &paths, &query, limit, explain, include_revoked, as_of)?;
    Ok(finalize_search_results(out, limit))
}

pub(super) fn search_paths(
    root: &Path,
    paths: &[PathBuf],
    query: &SearchText,
    limit: usize,
    explain: bool,
    include_revoked: bool,
    as_of: Option<NaiveDate>,
) -> Result<Vec<SearchResult>, String> {
    let workers = scan_worker_count(paths.len());
    let documents = unique_valid_documents_for_search(root, paths, workers)?;
    search_documents(
        &documents,
        query,
        limit,
        explain,
        include_revoked,
        as_of,
        workers,
    )
}

#[cfg(test)]
pub(super) fn search_paths_with_workers(
    root: &Path,
    paths: &[PathBuf],
    query: &SearchText,
    limit: usize,
    explain: bool,
    workers: usize,
) -> Result<Vec<SearchResult>, String> {
    let documents = unique_valid_documents_for_search(root, paths, workers)?;
    search_documents(&documents, query, limit, explain, false, None, workers)
}

fn search_documents(
    documents: &[Document],
    query: &SearchText,
    limit: usize,
    explain: bool,
    include_revoked: bool,
    as_of: Option<NaiveDate>,
    workers: usize,
) -> Result<Vec<SearchResult>, String> {
    let lexical = scan_chunks_with_workers(documents, workers, "search", |chunk| {
        search_chunk(chunk, query, None, limit, explain, include_revoked, as_of)
    })?
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if !lexical.is_empty() {
        return Ok(lexical);
    }
    let Some(query_embedding) = semantic::embed_query(query) else {
        return Ok(lexical);
    };
    Ok(
        scan_chunks_with_workers(documents, workers, "search", |chunk| {
            search_chunk(
                chunk,
                query,
                Some(query_embedding.as_slice()),
                limit,
                explain,
                include_revoked,
                as_of,
            )
        })?
        .into_iter()
        .flatten()
        .collect(),
    )
}

pub(super) fn unique_valid_documents_for_search(
    root: &Path,
    paths: &[PathBuf],
    workers: usize,
) -> Result<Vec<Document>, String> {
    let documents = scan_chunks_with_workers(paths, workers, "search", |chunk| {
        chunk
            .iter()
            .map(|path| load_local(root, path))
            .collect::<Vec<_>>()
    })?
    .into_iter()
    .flatten()
    .filter(document_is_valid)
    .collect::<Vec<_>>();
    let id_counts = document_id_counts(&documents);
    Ok(documents
        .into_iter()
        .filter(|document| id_counts.get(document.id.as_str()).copied() == Some(1))
        .collect())
}

pub(super) fn search_chunk(
    documents: &[Document],
    query: &SearchText,
    query_embedding: Option<&[f32]>,
    limit: usize,
    explain: bool,
    include_revoked: bool,
    as_of: Option<NaiveDate>,
) -> Vec<SearchResult> {
    let mut top = Vec::with_capacity(limit);
    for document in documents {
        if !include_revoked && wiki_or_lens_is_revoked(document) {
            continue;
        }
        if as_of.is_some_and(|cutoff| {
            DateTime::parse_from_rfc3339(&document.as_of)
                .ok()
                .is_none_or(|timestamp| timestamp.date_naive() > cutoff)
        }) {
            continue;
        }
        if let Some(candidate) = match query_embedding {
            Some(embedding) => semantic_document(document, embedding),
            None => search_document(document, query),
        } {
            retain_top_ranked_result(&mut top, candidate, limit);
        }
    }
    top.into_iter()
        .map(|mut item| {
            if explain {
                let (match_tier, matched_fields, matched_terms) =
                    explain_match(&item.document, &item.section, query);
                item.result.match_tier = match_tier;
                item.result.matched_fields = matched_fields;
                item.result.matched_terms = matched_terms;
                item.result.text.clear();
            }
            item.result
        })
        .collect()
}

pub(super) fn retain_top_ranked_result(
    top: &mut Vec<RankedSearchResult>,
    candidate: RankedSearchResult,
    limit: usize,
) {
    // A result below a chunk's top `limit` unique IDs cannot enter the global
    // top `limit`: that same chunk already contains `limit` distinct IDs ahead
    // of it. Keeping the best duplicate per ID preserves the final dedup order.
    if let Some(index) = top
        .iter()
        .position(|item| item.result.id == candidate.result.id)
    {
        if search_result_order(&candidate.result, &top[index].result).is_lt() {
            top[index] = candidate;
            top.sort_by(|a, b| search_result_order(&a.result, &b.result));
        }
        return;
    }
    top.push(candidate);
    top.sort_by(|a, b| search_result_order(&a.result, &b.result));
    top.truncate(limit);
}

pub(super) fn wiki_or_lens_is_revoked(doc: &Document) -> bool {
    matches!(doc.kind.as_str(), "wiki" | "lens") && doc.status.as_deref() == Some("revoked")
}

fn semantic_document(doc: &Document, query_embedding: &[f32]) -> Option<RankedSearchResult> {
    let score = semantic::score_document(doc, query_embedding)?;
    let document_level = Section {
        heading: String::new(),
        heading_path: vec![],
        text: String::new(),
    };
    Some(RankedSearchResult {
        result: search_result(doc, &document_level, score),
        document: doc.clone(),
        section: document_level,
    })
}

pub(super) fn search_document(doc: &Document, query: &SearchText) -> Option<RankedSearchResult> {
    if !document_is_valid(doc) {
        return None;
    }

    let document_relevance = document_relevance(doc, query);
    let document_numbers = document_sensitive_numbers(doc);
    let mut matched_section = false;
    let mut best = None;
    for section in split_sections(&doc.body) {
        let section_relevance = section_relevance(&section, query);
        let section_numbers = section_sensitive_numbers(&section);
        let numbers_match = query
            .sensitive_numbers
            .iter()
            .all(|number| document_numbers.contains(number) || section_numbers.contains(number));
        let complete_concise_document_match =
            !query.tokens.is_empty() && query.tokens.is_subset(&document_relevance.concise_terms);
        let weak_section_for_strong_document = (document_relevance.relevance.tier >= 3
            || complete_concise_document_match)
            && section_relevance.relevance.tier == 1;
        if section_relevance.relevance.is_match()
            && numbers_match
            && !weak_section_for_strong_document
        {
            matched_section = true;
            let relevance = combined_relevance(&document_relevance, &section_relevance, query);
            if has_minimum_query_coverage(relevance, &document_relevance, &section_relevance, query)
            {
                let result = search_result(doc, &section, relevance.score());
                if best
                    .as_ref()
                    .is_none_or(|(current, _)| search_result_order(&result, current).is_lt())
                {
                    best = Some((result, section));
                }
            }
        }
    }
    if !matched_section
        && document_relevance.relevance.is_match()
        && query.sensitive_numbers.is_subset(&document_numbers)
    {
        let document_level = Section {
            heading: String::new(),
            heading_path: vec![],
            text: String::new(),
        };
        let section_relevance = SectionRelevance::default();
        if has_minimum_query_coverage(
            document_relevance.relevance,
            &document_relevance,
            &section_relevance,
            query,
        ) {
            best = Some((
                search_result(doc, &document_level, document_relevance.relevance.score()),
                document_level,
            ));
        }
    }
    best.map(|(result, section)| RankedSearchResult {
        result,
        document: doc.clone(),
        section,
    })
}

pub(super) fn finalize_search_results(
    mut out: Vec<SearchResult>,
    limit: usize,
) -> Vec<SearchResult> {
    out.sort_by(search_result_order);
    let mut seen = BTreeSet::new();
    out.retain(|item| seen.insert(item.id.clone()));
    out.truncate(limit);
    out
}

pub(super) fn scan_worker_count(file_count: usize) -> usize {
    if file_count < PARALLEL_SCAN_MIN_FILES {
        return 1;
    }
    thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(MAX_SCAN_WORKERS)
        .min(file_count)
}

pub(super) fn scan_chunks<I, T, F>(items: &[I], operation: &str, scan: F) -> Result<Vec<T>, String>
where
    I: Sync,
    T: Send,
    F: Fn(&[I]) -> T + Sync,
{
    scan_chunks_with_workers(items, scan_worker_count(items.len()), operation, scan)
}

pub(super) fn scan_chunks_with_workers<I, T, F>(
    items: &[I],
    workers: usize,
    operation: &str,
    scan: F,
) -> Result<Vec<T>, String>
where
    I: Sync,
    T: Send,
    F: Fn(&[I]) -> T + Sync,
{
    let workers = workers.max(1).min(items.len().max(1));
    if workers == 1 {
        return Ok(vec![scan(items)]);
    }

    let chunk_size = items.len().div_ceil(workers);
    thread::scope(|scope| {
        let mut chunks = Vec::new();
        for chunk in items.chunks(chunk_size) {
            let scan = &scan;
            match thread::Builder::new().spawn_scoped(scope, move || scan(chunk)) {
                Ok(handle) => chunks.push(Ok(handle)),
                Err(_) => chunks.push(Err(scan(chunk))),
            }
        }

        let mut results = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            match chunk {
                Ok(handle) => match handle.join() {
                    Ok(result) => results.push(result),
                    Err(_) => return Err(format!("knowledge {operation} worker failed")),
                },
                Err(result) => results.push(result),
            }
        }
        Ok(results)
    })
}

pub(super) fn candidate_results(results: &[SearchResult]) -> Vec<CandidateResult> {
    results
        .iter()
        .map(|result| CandidateResult {
            id: result.id.clone(),
            kind: result.kind.clone(),
            title: result.title.clone(),
            summary: result.summary.clone(),
            as_of: result.as_of.clone(),
            section: result.section.clone(),
            heading_path: result.heading_path.clone(),
            entities: result.entities.clone(),
            period: result.period.clone(),
            aliases: result.aliases.clone(),
            score: result.score,
            match_tier: result.match_tier.clone(),
            matched_fields: result.matched_fields.clone(),
            matched_terms: result.matched_terms.clone(),
        })
        .collect()
}
