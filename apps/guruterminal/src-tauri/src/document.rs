//! Bounded Markdown extraction for Chat attachments and web fetches.
//!
//! The extractor is a derivative surface: callers retain a content digest and
//! the host registers the delivered result. Empty text-layer PDFs fail closed
//! so the agent cannot treat a scan as an empty document.

use std::io::Cursor;

use office_oxide::{Document, DocumentFormat};
use pdf_oxide::PdfDocument;
use thiserror::Error;
use zip::ZipArchive;

pub const EXTRACTED_PART_BYTES: usize = 512 * 1024;
pub const MAX_EXTRACTED_MARKDOWN_BYTES: usize = 2 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 4_096;
const MAX_ARCHIVE_ENTRY_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ARCHIVE_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PDF_PAGES: usize = 100;

#[derive(Debug, Error)]
pub enum DocumentError {
    #[error("document content type is not supported")]
    Unsupported,
    #[error("document bytes do not match the declared type")]
    TypeMismatch,
    #[error("document has no extractable text layer")]
    NoTextLayer,
    #[error("document text is not valid UTF-8")]
    InvalidText,
    #[error("document could not be parsed")]
    Parse,
    #[error("document archive exceeds its extraction budget")]
    ArchiveLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentKind {
    Pdf,
    Docx,
    Xlsx,
    Pptx,
    Doc,
    Xls,
    Ppt,
    Html,
    PlainText,
}

impl DocumentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
            Self::Pptx => "pptx",
            Self::Doc => "doc",
            Self::Xls => "xls",
            Self::Ppt => "ppt",
            Self::Html => "html",
            Self::PlainText => "text",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractedDocument {
    pub markdown: String,
    pub kind: DocumentKind,
    pub page_count: Option<u32>,
    pub truncated: bool,
}

pub fn extract(
    bytes: &[u8],
    media_type: &str,
    max_bytes: usize,
) -> Result<ExtractedDocument, DocumentError> {
    let media_type = media_type
        .split(';')
        .next()
        .unwrap_or(media_type)
        .trim()
        .to_ascii_lowercase();
    let kind = detect_kind(bytes, &media_type)?;
    let extracted = match kind {
        DocumentKind::Pdf => extract_pdf(bytes)?,
        DocumentKind::Docx
        | DocumentKind::Xlsx
        | DocumentKind::Pptx
        | DocumentKind::Doc
        | DocumentKind::Xls
        | DocumentKind::Ppt => extract_office(bytes, kind)?,
        DocumentKind::Html => extract_html_document(bytes)?,
        DocumentKind::PlainText => extract_plain(bytes)?,
    };
    if extracted.markdown.trim().is_empty() {
        return Err(DocumentError::NoTextLayer);
    }
    let (markdown, truncated) = truncate_utf8(&extracted.markdown, max_bytes);
    Ok(ExtractedDocument {
        markdown,
        kind: extracted.kind,
        page_count: extracted.page_count,
        truncated: extracted.truncated || truncated,
    })
}

pub fn extract_html(html: &str, fallback_title: &str) -> (String, String) {
    let (title, content_html) = match dom_smoothie::Readability::new(html, None, None)
        .ok()
        .and_then(|mut reader| reader.parse().ok())
    {
        Some(article) => {
            let title = article.title.trim();
            let title = if title.is_empty() {
                fallback_title.to_owned()
            } else {
                title.to_owned()
            };
            (title, article.content.to_string())
        }
        None => (fallback_title.to_owned(), html.to_owned()),
    };
    let content_html = strip_inert_html(&content_html);
    let markdown = htmd::convert(&content_html).unwrap_or_else(|_| {
        content_html
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    });
    (bounded_text(&title, 512), markdown)
}

pub fn split_extracted_markdown(markdown: &str, part_bytes: usize) -> Vec<String> {
    if markdown.len() <= part_bytes {
        return vec![markdown.to_owned()];
    }
    let mut parts = Vec::new();
    let mut rest = markdown;
    while !rest.is_empty() {
        let mut end = part_bytes.min(rest.len());
        while end > 0 && !rest.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            break;
        }
        parts.push(rest[..end].to_owned());
        rest = &rest[end..];
    }
    parts
}

pub fn looks_like_pdf(bytes: &[u8]) -> bool {
    bytes
        .get(..bytes.len().min(1_024))
        .is_some_and(|prefix| prefix.windows(5).any(|window| window == b"%PDF-"))
}

pub fn looks_like_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06")
}

pub fn looks_like_ole(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0])
}

fn detect_kind(bytes: &[u8], media_type: &str) -> Result<DocumentKind, DocumentError> {
    if looks_like_pdf(bytes) {
        if media_type_conflicts_with_pdf(media_type) {
            return Err(DocumentError::TypeMismatch);
        }
        return Ok(DocumentKind::Pdf);
    }
    if looks_like_zip(bytes) {
        if let Some(kind) = office_kind_from_zip(bytes)? {
            if media_type_conflicts_with_office(kind, media_type) {
                return Err(DocumentError::TypeMismatch);
            }
            return Ok(kind);
        }
        return Err(DocumentError::Unsupported);
    }
    if looks_like_ole(bytes) {
        return ole_kind(bytes, media_type)?.ok_or(DocumentError::Unsupported);
    }
    if is_html_media_type(media_type) || looks_like_html(bytes) {
        if media_type_conflicts_with_text(media_type) {
            return Err(DocumentError::TypeMismatch);
        }
        return Ok(DocumentKind::Html);
    }
    if is_plain_media_type(media_type) {
        return Ok(DocumentKind::PlainText);
    }
    Err(DocumentError::Unsupported)
}

fn media_type_conflicts_with_pdf(media_type: &str) -> bool {
    !matches!(
        media_type,
        "" | "application/pdf" | "application/octet-stream"
    )
}

fn media_type_conflicts_with_office(kind: DocumentKind, media_type: &str) -> bool {
    if matches!(
        media_type,
        "" | "application/zip" | "application/octet-stream"
    ) {
        return false;
    }
    !matches!(
        (kind, media_type),
        (
            DocumentKind::Docx,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        ) | (
            DocumentKind::Xlsx,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        ) | (
            DocumentKind::Pptx,
            "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        )
    )
}

fn media_type_conflicts_with_text(media_type: &str) -> bool {
    matches!(
        media_type,
        "application/pdf"
            | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
    )
}

fn is_html_media_type(media_type: &str) -> bool {
    matches!(media_type, "text/html" | "application/xhtml+xml")
}

fn is_plain_media_type(media_type: &str) -> bool {
    media_type.starts_with("text/")
        || matches!(
            media_type,
            "application/json" | "application/xml" | "application/csv" | "text/csv" | "text/xml"
        )
}

pub fn looks_like_html(bytes: &[u8]) -> bool {
    let without_bom = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let start = without_bom
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map(|index| &without_bom[index..])
        .unwrap_or(&[]);
    start.starts_with(b"<") || start.starts_with(b"<!DOCTYPE") || start.starts_with(b"<!doctype")
}

fn office_kind_from_zip(bytes: &[u8]) -> Result<Option<DocumentKind>, DocumentError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| DocumentError::Parse)?;
    let entry_count = archive.len();
    if entry_count > MAX_ARCHIVE_ENTRIES {
        return Err(DocumentError::ArchiveLimit);
    }
    let mut names = Vec::new();
    let mut total_uncompressed = 0_u64;
    for index in 0..entry_count {
        let (entry_size, name) = {
            let file = archive.by_index(index).map_err(|_| DocumentError::Parse)?;
            (file.size(), file.name().replace('\\', "/"))
        };
        total_uncompressed = total_uncompressed
            .checked_add(entry_size)
            .ok_or(DocumentError::ArchiveLimit)?;
        if !archive_budget_allows(entry_count, total_uncompressed, entry_size) {
            return Err(DocumentError::ArchiveLimit);
        }
        names.push(name);
    }
    Ok(if names.iter().any(|name| name == "word/document.xml") {
        Some(DocumentKind::Docx)
    } else if names.iter().any(|name| name == "xl/workbook.xml") {
        Some(DocumentKind::Xlsx)
    } else if names.iter().any(|name| name == "ppt/presentation.xml") {
        Some(DocumentKind::Pptx)
    } else {
        None
    })
}

fn archive_budget_allows(entry_count: usize, total_uncompressed: u64, entry_size: u64) -> bool {
    entry_count <= MAX_ARCHIVE_ENTRIES
        && entry_size <= MAX_ARCHIVE_ENTRY_BYTES
        && total_uncompressed <= MAX_ARCHIVE_UNCOMPRESSED_BYTES
}

fn ole_kind(bytes: &[u8], media_type: &str) -> Result<Option<DocumentKind>, DocumentError> {
    let declared = match media_type {
        "application/msword" => Some(DocumentKind::Doc),
        "application/vnd.ms-excel" => Some(DocumentKind::Xls),
        "application/vnd.ms-powerpoint" => Some(DocumentKind::Ppt),
        _ => None,
    };
    if declared.is_none() && !matches!(media_type, "" | "application/octet-stream") {
        return Err(DocumentError::TypeMismatch);
    }
    let detected = ole_kind_from_cfb(bytes)?;
    if declared.is_some() && declared != detected {
        return Err(DocumentError::TypeMismatch);
    }
    Ok(detected)
}

fn ole_kind_from_cfb(bytes: &[u8]) -> Result<Option<DocumentKind>, DocumentError> {
    let compound = cfb::CompoundFile::open(Cursor::new(bytes)).map_err(|_| DocumentError::Parse)?;
    let mut doc = false;
    let mut xls = false;
    let mut ppt = false;
    for (index, entry) in compound.walk().enumerate() {
        if index >= MAX_ARCHIVE_ENTRIES {
            return Err(DocumentError::ArchiveLimit);
        }
        match entry.name().to_ascii_lowercase().as_str() {
            "worddocument" => doc = true,
            "workbook" | "book" => xls = true,
            "powerpoint document" => ppt = true,
            _ => {}
        }
    }
    match (doc, xls, ppt) {
        (true, false, false) => Ok(Some(DocumentKind::Doc)),
        (false, true, false) => Ok(Some(DocumentKind::Xls)),
        (false, false, true) => Ok(Some(DocumentKind::Ppt)),
        (false, false, false) => Ok(None),
        _ => Err(DocumentError::TypeMismatch),
    }
}

fn extract_pdf(bytes: &[u8]) -> Result<ExtractedDocument, DocumentError> {
    let document = PdfDocument::from_bytes(bytes.to_vec()).map_err(|_| DocumentError::Parse)?;
    let page_count = document.page_count().map_err(|_| DocumentError::Parse)?;
    let extracted_pages = page_count.min(MAX_PDF_PAGES);
    let mut pages = Vec::with_capacity(extracted_pages);
    for index in 0..extracted_pages {
        pages.push(
            document
                .extract_text(index)
                .map_err(|_| DocumentError::Parse)?,
        );
    }
    let markdown = pages.join("\n\n");
    if markdown.trim().is_empty() {
        return Err(DocumentError::NoTextLayer);
    }
    Ok(ExtractedDocument {
        markdown,
        kind: DocumentKind::Pdf,
        page_count: Some(u32::try_from(page_count).map_err(|_| DocumentError::ArchiveLimit)?),
        truncated: page_count > MAX_PDF_PAGES,
    })
}

fn strip_inert_html(html: &str) -> String {
    let mut sanitized = strip_html_comments(html);
    for tag in ["script", "style", "noscript", "template"] {
        sanitized = strip_html_element(&sanitized, tag);
    }
    sanitized
}

fn strip_html_comments(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<!--") {
        output.push_str(&rest[..start]);
        let after_start = &rest[start + 4..];
        let Some(end) = after_start.find("-->") else {
            return output;
        };
        rest = &after_start[end + 3..];
    }
    output.push_str(rest);
    output
}

fn strip_html_element(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}");
    let mut output = String::with_capacity(html.len());
    let mut cursor = 0;
    while let Some(relative_start) = lower[cursor..].find(&open) {
        let start = cursor + relative_start;
        let name_end = start + open.len();
        if lower
            .as_bytes()
            .get(name_end)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'>' | b'/'))
        {
            output.push_str(&html[cursor..name_end]);
            cursor = name_end;
            continue;
        }
        output.push_str(&html[cursor..start]);
        let Some(relative_close) = lower[name_end..].find(&close) else {
            return output;
        };
        let close_start = name_end + relative_close;
        let Some(relative_end) = lower[close_start..].find('>') else {
            return output;
        };
        cursor = close_start + relative_end + 1;
    }
    output.push_str(&html[cursor..]);
    output
}

fn extract_office(bytes: &[u8], kind: DocumentKind) -> Result<ExtractedDocument, DocumentError> {
    let format = match kind {
        DocumentKind::Docx => DocumentFormat::Docx,
        DocumentKind::Xlsx => DocumentFormat::Xlsx,
        DocumentKind::Pptx => DocumentFormat::Pptx,
        DocumentKind::Doc => DocumentFormat::Doc,
        DocumentKind::Xls => DocumentFormat::Xls,
        DocumentKind::Ppt => DocumentFormat::Ppt,
        _ => return Err(DocumentError::Unsupported),
    };
    let document = Document::from_reader(Cursor::new(bytes.to_vec()), format)
        .map_err(|_| DocumentError::Parse)?;
    let markdown = document.to_markdown();
    if markdown.trim().is_empty() {
        return Err(DocumentError::NoTextLayer);
    }
    Ok(ExtractedDocument {
        markdown,
        kind,
        page_count: None,
        truncated: false,
    })
}

fn extract_html_document(bytes: &[u8]) -> Result<ExtractedDocument, DocumentError> {
    let html = std::str::from_utf8(bytes).map_err(|_| DocumentError::InvalidText)?;
    if html.contains('\0') {
        return Err(DocumentError::InvalidText);
    }
    let (_title, markdown) = extract_html(html, "Untitled document");
    Ok(ExtractedDocument {
        markdown,
        kind: DocumentKind::Html,
        page_count: None,
        truncated: false,
    })
}

fn extract_plain(bytes: &[u8]) -> Result<ExtractedDocument, DocumentError> {
    let text = std::str::from_utf8(bytes).map_err(|_| DocumentError::InvalidText)?;
    if text.contains('\0') {
        return Err(DocumentError::InvalidText);
    }
    Ok(ExtractedDocument {
        markdown: text.to_owned(),
        kind: DocumentKind::PlainText,
        page_count: None,
        truncated: false,
    })
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    truncate_utf8(value.trim(), max_bytes).0
}

fn truncate_utf8(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_owned(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_owned(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use office_oxide::create::create_from_markdown_to_writer;
    use std::io::Write;

    fn minimal_pdf(text: &str) -> Vec<u8> {
        let escaped = text
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        let stream = if escaped.is_empty() {
            "BT ET".to_owned()
        } else {
            format!("BT /F1 12 Tf 72 720 Td ({escaped}) Tj ET")
        };
        let objects = [
            "1 0 obj<< /Type /Catalog /Pages 2 0 R >>endobj\n".to_string(),
            "2 0 obj<< /Type /Pages /Kids [3 0 R] /Count 1 >>endobj\n".to_string(),
            "3 0 obj<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>endobj\n".to_string(),
            format!(
                "4 0 obj<< /Length {} >>stream\n{}\nendstream\nendobj\n",
                stream.len(),
                stream
            ),
            "5 0 obj<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>endobj\n".to_string(),
        ];
        let header = "%PDF-1.4\n";
        let mut body = String::new();
        let mut offsets = Vec::new();
        let mut pos = header.len();
        for object in &objects {
            offsets.push(pos);
            body.push_str(object);
            pos += object.len();
        }
        let mut xref = String::from("xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            xref.push_str(&format!("{offset:010} 00000 n \n"));
        }
        format!("{header}{body}{xref}trailer<< /Size 6 /Root 1 0 R >>\nstartxref\n{pos}\n%%EOF\n")
            .into_bytes()
    }

    fn minimal_docx(text: &str) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut cursor);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("[Content_Types].xml", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#,
            )
            .unwrap();
            zip.start_file("_rels/.rels", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#,
            )
            .unwrap();
            zip.start_file("word/document.xml", options).unwrap();
            zip.write_all(
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body></w:document>"#
                )
                .as_bytes(),
            )
            .unwrap();
            zip.finish().unwrap();
        }
        cursor.into_inner()
    }

    fn modern_office(format: DocumentFormat) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        create_from_markdown_to_writer(
            "# Quarterly report\n\nRevenue rose to 42.\n\n| Metric | Value |\n| --- | ---: |\n| Revenue | 42 |",
            format,
            &mut cursor,
        )
        .unwrap();
        cursor.into_inner()
    }

    fn legacy_office_container(stream_name: &str) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut compound = cfb::CompoundFile::create(cursor).unwrap();
        {
            let mut stream = compound.create_stream(format!("/{stream_name}")).unwrap();
            stream.write_all(b"fixture").unwrap();
        }
        compound.into_inner().into_inner()
    }

    #[test]
    fn extracts_text_layer_pdf() {
        let bytes = minimal_pdf("Hello Guru");
        let extracted = extract(&bytes, "application/pdf", 8 * 1024).unwrap();
        assert_eq!(extracted.kind, DocumentKind::Pdf);
        assert_eq!(extracted.page_count, Some(1));
        assert!(extracted.markdown.contains("Hello Guru"));
        assert!(!extracted.truncated);
    }

    #[test]
    fn rejects_pdf_without_a_text_layer() {
        let bytes = minimal_pdf("");
        assert!(matches!(
            extract(&bytes, "application/pdf", 8 * 1024),
            Err(DocumentError::NoTextLayer)
        ));
    }

    #[test]
    fn rejects_magic_byte_mismatch_for_declared_pdf() {
        assert!(matches!(
            extract(b"not a document", "application/pdf", 1024),
            Err(DocumentError::Unsupported | DocumentError::TypeMismatch)
        ));
    }

    #[test]
    fn detects_pdf_header_within_the_first_kib_and_rejects_hard_type_conflicts() {
        let mut bytes = b"harmless leading bytes\n".to_vec();
        bytes.extend(minimal_pdf("Header offset"));
        assert!(looks_like_pdf(&bytes));
        assert!(matches!(
            detect_kind(&bytes, "text/html"),
            Err(DocumentError::TypeMismatch)
        ));
    }

    #[test]
    fn extracts_docx_from_zip_magic_bytes() {
        let bytes = minimal_docx("Quarterly revenue rose.");
        let extracted = extract(
            &bytes,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            8 * 1024,
        )
        .unwrap();
        assert_eq!(extracted.kind, DocumentKind::Docx);
        assert!(extracted.markdown.contains("Quarterly revenue rose."));
    }

    #[test]
    fn extracts_every_modern_office_format_from_zip_magic_bytes() {
        for (format, kind, media_type) in [
            (
                DocumentFormat::Docx,
                DocumentKind::Docx,
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ),
            (
                DocumentFormat::Xlsx,
                DocumentKind::Xlsx,
                "application/octet-stream",
            ),
            (DocumentFormat::Pptx, DocumentKind::Pptx, "application/zip"),
        ] {
            let extracted = extract(&modern_office(format), media_type, 64 * 1024).unwrap();
            assert_eq!(extracted.kind, kind);
            assert!(extracted.markdown.contains("Revenue"));
            assert!(extracted.markdown.contains("42"));
        }
    }

    #[test]
    fn rejects_office_zip_declared_as_pdf() {
        let bytes = modern_office(DocumentFormat::Docx);
        assert!(matches!(
            extract(&bytes, "application/pdf", 64 * 1024),
            Err(DocumentError::TypeMismatch)
        ));
    }

    #[test]
    fn detects_legacy_office_from_cfb_streams_when_mime_is_generic() {
        for (stream_name, kind) in [
            ("WordDocument", DocumentKind::Doc),
            ("Workbook", DocumentKind::Xls),
            ("PowerPoint Document", DocumentKind::Ppt),
        ] {
            let bytes = legacy_office_container(stream_name);
            assert_eq!(
                detect_kind(&bytes, "application/octet-stream").unwrap(),
                kind
            );
        }
        let bytes = legacy_office_container("Workbook");
        assert!(matches!(
            detect_kind(&bytes, "application/msword"),
            Err(DocumentError::TypeMismatch)
        ));
    }

    #[test]
    fn rejects_office_archives_outside_the_decompression_budget() {
        assert!(archive_budget_allows(1, 1024, 1024));
        assert!(!archive_budget_allows(MAX_ARCHIVE_ENTRIES + 1, 0, 0));
        assert!(!archive_budget_allows(
            1,
            MAX_ARCHIVE_UNCOMPRESSED_BYTES + 1,
            1
        ));
        assert!(!archive_budget_allows(
            1,
            MAX_ARCHIVE_ENTRY_BYTES + 1,
            MAX_ARCHIVE_ENTRY_BYTES + 1
        ));
    }

    #[test]
    fn extracts_html_tables_as_markdown() {
        let html = r#"<html><head><title>Report</title><script>ignore()</script></head>
<body><nav>Skip</nav><article><h1>Heading</h1><p>Useful <b>text</b>.</p>
<table><tr><th>Year</th><th>Revenue</th></tr><tr><td>2024</td><td>10</td></tr></table>
</article></body></html>"#;
        let extracted = extract(html.as_bytes(), "text/html", 8 * 1024).unwrap();
        assert_eq!(extracted.kind, DocumentKind::Html);
        assert!(extracted.markdown.contains("Useful"));
        assert!(extracted.markdown.contains("text"));
        assert!(!extracted.markdown.contains("ignore()"));
        assert!(
            extracted.markdown.contains('|')
                || extracted.markdown.contains("Revenue")
                || extracted.markdown.contains("2024")
        );
    }

    #[test]
    fn strips_inert_html_even_when_readability_has_no_article() {
        let html = "<html><body><script>attack()</script><style>.secret{}</style><template>hidden prompt</template><p>Safe text</p></body></html>";
        let sanitized = strip_inert_html(html);
        assert!(sanitized.contains("Safe text"));
        assert!(!sanitized.contains("attack"));
        assert!(!sanitized.contains("secret"));
        assert!(!sanitized.contains("hidden prompt"));
        let (_, markdown) = extract_html(html, "fallback");
        assert!(!markdown.contains("attack"));
        assert!(!markdown.contains("hidden prompt"));
    }

    #[test]
    fn truncates_extracted_markdown_on_the_byte_budget() {
        let bytes = minimal_pdf("ABCDEFGHIJ");
        let extracted = extract(&bytes, "application/pdf", 8).unwrap();
        assert!(extracted.truncated);
        assert!(extracted.markdown.len() <= 8);
    }

    #[test]
    fn splits_extracted_markdown_on_char_boundaries() {
        let parts = split_extracted_markdown("abcdef", 4);
        assert_eq!(parts, vec!["abcd".to_string(), "ef".to_string()]);
        let wide = split_extracted_markdown("한글본문", 4);
        assert!(wide.iter().all(|part| part.is_char_boundary(part.len())));
    }
}
