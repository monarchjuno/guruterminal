use super::*;

pub fn context(root: &Path) -> Result<KnowledgeContext, String> {
    let capture = capture_local_catalog(root)?;
    let check = check_documents_with_layout(&capture.documents, capture.layout_issues);
    let records = capture
        .documents
        .iter()
        .filter(|document| document_is_valid(document))
        .cloned()
        .collect::<Vec<_>>();
    let health = health_documents(&records, None);
    let charter = charter_from_records(&records);
    Ok(KnowledgeContext {
        check,
        health,
        revision: capture.revision,
        records,
        charter,
    })
}

fn charter_from_records(records: &[Document]) -> Option<KnowledgeCharterRead> {
    let mut matches = records
        .iter()
        .filter(|document| document.id == "lens:charter");
    let document = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(KnowledgeCharterRead {
        document: document.clone(),
        section: None,
        content: document.content.clone(),
    })
}
