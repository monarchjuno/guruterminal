//! Official representation matchers for known public Wikipedia, Wikidata, and
//! DOI URLs.
//!
//! Matcher and bounded JSON-projection patterns are ported from Oh My Pi
//! (`can1357/oh-my-pi` commit `76a294cb19bfded1e32e2111f1f729129595bf5e`),
//! copyright (c) can1357 and contributors, MIT License.
//!
//! Guru Terminal materializes one official representation's exact bytes per
//! `web.fetch`. It does not merge multiple API responses, auto-fetch
//! alternates, or keep OMP's site-handler registry.

use reqwest::Url;
use serde_json::Value;

use crate::document;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OfficialProvider {
    Wikipedia,
    Wikidata,
    Crossref,
}

impl OfficialProvider {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Wikipedia => "wikipedia",
            Self::Wikidata => "wikidata",
            Self::Crossref => "crossref",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct OfficialTarget {
    pub(super) provider: OfficialProvider,
    pub(super) record_id: String,
    pub(super) representation_url: Url,
}

const WIKIPEDIA_SKIP_PREFIXES: &[&str] = &[
    "special:",
    "file:",
    "image:",
    "mediawiki:",
    "template:",
    "help:",
    "category:",
    "talk:",
    "user:",
    "user_talk:",
    "wikipedia:",
    "portal:",
    "draft:",
    "module:",
    "timedtext:",
    "media:",
];

const WIKIDATA_PROPERTY_LABELS: &[(&str, &str)] = &[
    ("P31", "Instance of"),
    ("P279", "Subclass of"),
    ("P17", "Country"),
    ("P571", "Founded"),
    ("P576", "Dissolved"),
    ("P169", "CEO"),
    ("P112", "Founded by"),
    ("P159", "Headquarters"),
    ("P452", "Industry"),
    ("P856", "Website"),
    ("P569", "Born"),
    ("P570", "Died"),
    ("P106", "Occupation"),
    ("P577", "Publication date"),
    ("P50", "Author"),
    ("P123", "Publisher"),
    ("P178", "Developer"),
    ("P275", "License"),
    ("P348", "Version"),
];

pub(super) fn match_official(url: &Url) -> Option<OfficialTarget> {
    match_wikipedia(url)
        .or_else(|| match_wikidata(url))
        .or_else(|| match_doi(url))
}

pub(super) fn verified_record_id(target: &OfficialTarget, bytes: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(bytes).ok()?;
    match target.provider {
        OfficialProvider::Wikipedia => {
            let parsed_title = value
                .get("parse")
                .and_then(|parse| parse.get("title"))
                .and_then(Value::as_str)?;
            titles_match(&target.record_id, parsed_title).then(|| parsed_title.to_owned())
        }
        OfficialProvider::Wikidata => {
            let qid = target.record_id.to_ascii_uppercase();
            let entities = value.get("entities")?.as_object()?;
            let entity = entities.get(&qid).or_else(|| {
                entities
                    .values()
                    .find(|entity| entity.get("id").and_then(Value::as_str) == Some(qid.as_str()))
            })?;
            let id = entity
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or(qid.as_str());
            (id.eq_ignore_ascii_case(&qid)).then(|| id.to_ascii_uppercase())
        }
        OfficialProvider::Crossref => {
            let doi = value
                .get("message")
                .and_then(|message| message.get("DOI"))
                .and_then(Value::as_str)?;
            dois_match(&target.record_id, doi).then(|| normalize_doi(doi))
        }
    }
}

pub(super) fn project_markdown(target: &OfficialTarget, bytes: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(bytes).ok()?;
    match target.provider {
        OfficialProvider::Wikipedia => project_wikipedia(&value),
        OfficialProvider::Wikidata => project_wikidata(&value, &target.record_id),
        OfficialProvider::Crossref => project_crossref(&value),
    }
}

fn match_wikipedia(url: &Url) -> Option<OfficialTarget> {
    let lang = wikipedia_lang(url.host_str()?)?;
    let title = wiki_title(url)?;
    if is_skipped_wiki_title(&title) {
        return None;
    }
    let mut representation_url =
        Url::parse(&format!("https://{lang}.wikipedia.org/w/api.php")).ok()?;
    representation_url
        .query_pairs_mut()
        .append_pair("action", "parse")
        .append_pair("format", "json")
        .append_pair("formatversion", "2")
        .append_pair("prop", "text|displaytitle")
        .append_pair("page", &title.replace(' ', "_"));
    Some(OfficialTarget {
        provider: OfficialProvider::Wikipedia,
        record_id: title,
        representation_url,
    })
}

fn match_wikidata(url: &Url) -> Option<OfficialTarget> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if host != "www.wikidata.org" && host != "wikidata.org" && host != "m.wikidata.org" {
        return None;
    }
    let qid = wikidata_qid(url)?;
    let representation_url = Url::parse(&format!(
        "https://www.wikidata.org/wiki/Special:EntityData/{qid}.json"
    ))
    .ok()?;
    Some(OfficialTarget {
        provider: OfficialProvider::Wikidata,
        record_id: qid,
        representation_url,
    })
}

fn match_doi(url: &Url) -> Option<OfficialTarget> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if host != "doi.org" && host != "www.doi.org" && host != "dx.doi.org" {
        return None;
    }
    let doi = doi_from_path(url.path())?;
    let mut representation_url = Url::parse("https://api.crossref.org").ok()?;
    representation_url
        .path_segments_mut()
        .ok()?
        .extend(["works", &doi]);
    Some(OfficialTarget {
        provider: OfficialProvider::Crossref,
        record_id: doi,
        representation_url,
    })
}

fn wikipedia_lang(host: &str) -> Option<String> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let rest = host.strip_suffix(".wikipedia.org")?;
    if rest.is_empty() {
        return None;
    }
    let lang = rest.strip_suffix(".m").unwrap_or(rest);
    if lang.is_empty()
        || lang == "www"
        || lang == "m"
        || lang.contains('.')
        || !lang
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return None;
    }
    Some(lang.to_owned())
}

fn wiki_title(url: &Url) -> Option<String> {
    let mut segments = url.path_segments()?;
    if segments.next()? != "wiki" {
        return None;
    }
    let rest = segments.collect::<Vec<_>>();
    if rest.is_empty() || rest.iter().all(|segment| segment.is_empty()) {
        return None;
    }
    let title = rest
        .into_iter()
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    let title = percent_decode(&title)?;
    let title = title.replace('_', " ");
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

fn is_skipped_wiki_title(title: &str) -> bool {
    let key = title.replace(' ', "_").to_ascii_lowercase();
    WIKIPEDIA_SKIP_PREFIXES
        .iter()
        .any(|prefix| key.starts_with(prefix))
}

fn wikidata_qid(url: &Url) -> Option<String> {
    let mut segments = url.path_segments()?;
    let first = segments.next()?;
    let candidate = match first {
        "wiki" | "entity" => segments.next()?,
        _ => return None,
    };
    parse_qid(candidate)
}

fn parse_qid(value: &str) -> Option<String> {
    let value = value.trim();
    let (prefix, digits) = value.split_at(value.len().min(1));
    if !prefix.eq_ignore_ascii_case("Q")
        || digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some(format!("Q{digits}"))
}

fn doi_from_path(path: &str) -> Option<String> {
    let raw = path.trim_start_matches('/');
    if raw.is_empty() {
        return None;
    }
    let decoded = percent_decode(raw)?;
    let stripped = decoded
        .trim()
        .trim_start_matches('/')
        .trim_start_matches("doi:")
        .trim_start_matches("DOI:");
    let doi = normalize_doi(stripped);
    (doi.starts_with("10.") && doi.contains('/')).then_some(doi)
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_value(*bytes.get(index + 1)?)?;
            let low = hex_value(*bytes.get(index + 2)?)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(decoded).ok()?;
    (!decoded.chars().any(char::is_control)).then_some(decoded)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn normalize_doi(value: &str) -> String {
    value.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn titles_match(requested: &str, parsed: &str) -> bool {
    normalize_wiki_title(requested) == normalize_wiki_title(parsed)
}

fn normalize_wiki_title(title: &str) -> String {
    title
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn dois_match(requested: &str, parsed: &str) -> bool {
    normalize_doi(requested) == normalize_doi(parsed)
}

fn project_wikipedia(value: &Value) -> Option<String> {
    let parse = value.get("parse")?;
    let title = parse
        .get("displaytitle")
        .and_then(Value::as_str)
        .map(strip_markup)
        .filter(|title| !title.is_empty())
        .or_else(|| {
            parse
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "Wikipedia".to_owned());
    let html = parse.get("text").and_then(Value::as_str).unwrap_or("");
    if html.trim().is_empty() {
        return None;
    }
    let (_ignored, markdown) = document::extract_html(html, &title);
    let markdown = markdown.trim();
    if markdown.is_empty() {
        return None;
    }
    Some(format!("# {title}\n\n{markdown}"))
}

fn project_wikidata(value: &Value, qid: &str) -> Option<String> {
    let qid = qid.to_ascii_uppercase();
    let entities = value.get("entities")?.as_object()?;
    let entity = entities.get(&qid)?;
    let label = localized_text(entity.get("labels")).unwrap_or_else(|| qid.clone());
    let description = localized_text(entity.get("descriptions"));
    let aliases = localized_aliases(entity.get("aliases"));
    let mut markdown = format!("# {label} ({qid})\n\n");
    if let Some(description) = description {
        markdown.push_str(&format!("*{description}*\n\n"));
    }
    if !aliases.is_empty() {
        markdown.push_str(&format!("**Also known as:** {}\n\n", aliases.join(", ")));
    }
    if let Some(claims) = entity.get("claims").and_then(Value::as_object) {
        let mut properties = Vec::new();
        for (prop_id, claim_list) in claims {
            let Some(claim_list) = claim_list.as_array() else {
                continue;
            };
            let values = claim_list
                .iter()
                .filter_map(claim_value)
                .take(10)
                .collect::<Vec<_>>();
            if values.is_empty() {
                continue;
            }
            let label = WIKIDATA_PROPERTY_LABELS
                .iter()
                .find(|(id, _)| *id == prop_id.as_str())
                .map(|(_, label)| (*label).to_owned())
                .unwrap_or_else(|| prop_id.clone());
            properties.push(format!("- **{label}:** {}", values.join(", ")));
            if properties.len() >= 50 {
                break;
            }
        }
        if !properties.is_empty() {
            markdown.push_str("## Properties\n\n");
            markdown.push_str(&properties.join("\n"));
            markdown.push('\n');
        }
    }
    Some(markdown)
}

fn project_crossref(value: &Value) -> Option<String> {
    let message = value.get("message")?;
    let title = message
        .get("title")
        .and_then(Value::as_array)
        .and_then(|titles| titles.first())
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Crossref record");
    let mut markdown = format!("# {title}\n\n");
    if let Some(authors) = crossref_authors(message.get("author")) {
        markdown.push_str(&format!("**Authors:** {authors}\n"));
    }
    if let Some(journal) = first_string(message.get("container-title"))
        .or_else(|| first_string(message.get("short-container-title")))
    {
        markdown.push_str(&format!("**Journal:** {journal}\n"));
    }
    if let Some(publisher) = message.get("publisher").and_then(Value::as_str) {
        markdown.push_str(&format!("**Publisher:** {publisher}\n"));
    }
    if let Some(published) = crossref_date(message) {
        markdown.push_str(&format!("**Published:** {published}\n"));
    }
    if let Some(doi) = message.get("DOI").and_then(Value::as_str) {
        markdown.push_str(&format!("**DOI:** {}\n", normalize_doi(doi)));
    }
    if let Some(kind) = message.get("type").and_then(Value::as_str) {
        markdown.push_str(&format!("**Type:** {}\n", kind.replace('-', " ")));
    }
    markdown.push_str("\n## Abstract\n\n");
    markdown.push_str(&crossref_abstract(message.get("abstract")));
    markdown.push('\n');
    Some(markdown)
}

fn localized_text(value: Option<&Value>) -> Option<String> {
    let map = value?.as_object()?;
    map.get("en")
        .or_else(|| map.values().next())
        .and_then(|entry| entry.get("value"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn localized_aliases(value: Option<&Value>) -> Vec<String> {
    let Some(map) = value.and_then(Value::as_object) else {
        return Vec::new();
    };
    map.get("en")
        .or_else(|| map.values().next())
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .get("value")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .take(12)
        .collect()
}

fn claim_value(claim: &Value) -> Option<String> {
    let snak = claim.get("mainsnak")?;
    if snak.get("snaktype").and_then(Value::as_str) != Some("value") {
        return None;
    }
    if claim.get("rank").and_then(Value::as_str) == Some("deprecated") {
        return None;
    }
    let datavalue = snak.get("datavalue")?;
    let kind = datavalue.get("type").and_then(Value::as_str)?;
    let value = datavalue.get("value")?;
    match kind {
        "string" => value.as_str().map(str::to_owned),
        "wikibase-entityid" => value.get("id").and_then(Value::as_str).map(str::to_owned),
        "time" => value
            .get("time")
            .and_then(Value::as_str)
            .map(|time| time.trim_start_matches('+').chars().take(10).collect()),
        "quantity" => value
            .get("amount")
            .and_then(Value::as_str)
            .map(|amount| amount.trim_start_matches('+').to_owned()),
        "monolingualtext" => value.get("text").and_then(Value::as_str).map(str::to_owned),
        "globecoordinate" => {
            let lat = value.get("latitude")?.as_f64()?;
            let lon = value.get("longitude")?.as_f64()?;
            Some(format!("{lat:.4}, {lon:.4}"))
        }
        _ => None,
    }
}

fn first_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(Value::as_str)
}

fn crossref_authors(value: Option<&Value>) -> Option<String> {
    let names = value?
        .as_array()?
        .iter()
        .filter_map(|author| {
            if let Some(name) = author.get("name").and_then(Value::as_str) {
                return Some(name.to_owned());
            }
            let given = author.get("given").and_then(Value::as_str).unwrap_or("");
            let family = author.get("family").and_then(Value::as_str).unwrap_or("");
            let combined = format!("{given} {family}").trim().to_owned();
            (!combined.is_empty()).then_some(combined)
        })
        .collect::<Vec<_>>();
    (!names.is_empty()).then_some(names.join(", "))
}

fn crossref_date(message: &Value) -> Option<String> {
    [
        "published",
        "published-print",
        "published-online",
        "issued",
        "created",
    ]
    .into_iter()
    .find_map(|key| {
        message
            .get(key)
            .and_then(|date| date.get("date-parts"))
            .and_then(Value::as_array)
            .and_then(|parts| parts.first())
            .and_then(Value::as_array)
            .and_then(|parts| {
                let year = parts.first()?.as_u64()?;
                let mut formatted = year.to_string();
                if let Some(month) = parts.get(1).and_then(Value::as_u64) {
                    formatted.push_str(&format!("-{month:02}"));
                }
                if let Some(day) = parts.get(2).and_then(Value::as_u64) {
                    formatted.push_str(&format!("-{day:02}"));
                }
                Some(formatted)
            })
    })
}

fn crossref_abstract(value: Option<&Value>) -> String {
    let Some(abstract_text) = value.and_then(Value::as_str).map(str::trim) else {
        return "No abstract available.".to_owned();
    };
    if abstract_text.is_empty() {
        return "No abstract available.".to_owned();
    }
    if abstract_text.contains('<') {
        let (_title, markdown) = document::extract_html(abstract_text, "Abstract");
        let markdown = markdown.trim();
        if markdown.is_empty() {
            "No abstract available.".to_owned()
        } else {
            markdown.to_owned()
        }
    } else {
        abstract_text.to_owned()
    }
}

fn strip_markup(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut in_tag = false;
    for character in value.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(character),
            _ => {}
        }
    }
    html_unescape(&output)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn html_unescape(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_wikipedia_article_urls_to_mediawiki_parse() {
        let url = Url::parse("https://en.wikipedia.org/wiki/Albert_Einstein").unwrap();
        let target = match_official(&url).unwrap();
        assert_eq!(target.provider, OfficialProvider::Wikipedia);
        assert_eq!(target.record_id, "Albert Einstein");
        assert_eq!(
            target.representation_url.host_str(),
            Some("en.wikipedia.org")
        );
        assert_eq!(target.representation_url.path(), "/w/api.php");
        let query = target.representation_url.query().unwrap();
        assert!(query.contains("action=parse"));
        assert!(query.contains("page=Albert_Einstein"));
        assert!(match_official(
            &Url::parse("https://en.wikipedia.org/wiki/Special:Search").unwrap()
        )
        .is_none());
        assert!(match_official(
            &Url::parse("https://www.wikipedia.org/wiki/Albert_Einstein").unwrap()
        )
        .is_none());
        let percent = match_official(
            &Url::parse("https://en.wikipedia.org/wiki/100%25_renewable_energy").unwrap(),
        )
        .unwrap();
        assert_eq!(percent.record_id, "100% renewable energy");
        assert!(percent
            .representation_url
            .query()
            .unwrap()
            .contains("page=100%25_renewable_energy"));
    }

    #[test]
    fn maps_mobile_wikipedia_and_wikidata_entity_urls() {
        let wiki =
            match_official(&Url::parse("https://ko.m.wikipedia.org/wiki/서울").unwrap()).unwrap();
        assert_eq!(wiki.provider, OfficialProvider::Wikipedia);
        assert_eq!(wiki.record_id, "서울");
        assert_eq!(wiki.representation_url.host_str(), Some("ko.wikipedia.org"));
        assert!(match_official(
            &Url::parse("https://en.wikipedia.org/wiki/Special%3ASearch").unwrap()
        )
        .is_none());

        let entity =
            match_official(&Url::parse("https://www.wikidata.org/wiki/Q42").unwrap()).unwrap();
        assert_eq!(entity.provider, OfficialProvider::Wikidata);
        assert_eq!(entity.record_id, "Q42");
        assert_eq!(
            entity.representation_url.as_str(),
            "https://www.wikidata.org/wiki/Special:EntityData/Q42.json"
        );
        assert_eq!(
            match_official(&Url::parse("https://www.wikidata.org/entity/q5").unwrap())
                .unwrap()
                .record_id,
            "Q5"
        );
    }

    #[test]
    fn maps_doi_urls_to_crossref_works() {
        let target =
            match_official(&Url::parse("https://doi.org/10.1038/nature12373").unwrap()).unwrap();
        assert_eq!(target.provider, OfficialProvider::Crossref);
        assert_eq!(target.record_id, "10.1038/nature12373");
        assert_eq!(
            target.representation_url.host_str(),
            Some("api.crossref.org")
        );
        assert!(target.representation_url.path().contains("nature12373"));
        assert!(
            match_official(&Url::parse("https://example.com/10.1038/nature12373").unwrap())
                .is_none()
        );
        let encoded =
            match_official(&Url::parse("https://doi.org/10.1000%2FABC%28test%29").unwrap())
                .unwrap();
        assert_eq!(encoded.record_id, "10.1000/abc(test)");
    }

    #[test]
    fn fails_closed_on_identity_mismatch() {
        let wiki =
            match_official(&Url::parse("https://en.wikipedia.org/wiki/USA").unwrap()).unwrap();
        let mismatched = br#"{"parse":{"title":"United States","text":"<p>USA</p>"}}"#;
        assert!(verified_record_id(&wiki, mismatched).is_none());
        let matched = br#"{"parse":{"title":"USA","text":"<p>USA</p>"}}"#;
        assert_eq!(verified_record_id(&wiki, matched).as_deref(), Some("USA"));

        let entity =
            match_official(&Url::parse("https://www.wikidata.org/wiki/Q42").unwrap()).unwrap();
        assert!(verified_record_id(&entity, br#"{"entities":{"Q99":{"id":"Q99"}}}"#).is_none());
        assert_eq!(
            verified_record_id(&entity, br#"{"entities":{"Q42":{"id":"Q42"}}}"#).as_deref(),
            Some("Q42")
        );

        let doi =
            match_official(&Url::parse("https://doi.org/10.1038/nature12373").unwrap()).unwrap();
        assert!(verified_record_id(&doi, br#"{"message":{"DOI":"10.1038/other"}}"#).is_none());
        assert_eq!(
            verified_record_id(&doi, br#"{"message":{"DOI":"10.1038/NATURE12373"}}"#).as_deref(),
            Some("10.1038/nature12373")
        );
    }

    #[test]
    fn projects_bounded_markdown_from_one_representation() {
        let wiki =
            match_official(&Url::parse("https://en.wikipedia.org/wiki/Ada_Lovelace").unwrap())
                .unwrap();
        let markdown = project_markdown(
            &wiki,
            br#"{"parse":{"title":"Ada Lovelace","displaytitle":"Ada Lovelace","text":"<p>Ada Lovelace wrote the first algorithm.</p>"}}"#,
        )
        .unwrap();
        assert!(markdown.contains("# Ada Lovelace"));
        assert!(markdown.contains("first algorithm"));

        let entity =
            match_official(&Url::parse("https://www.wikidata.org/wiki/Q42").unwrap()).unwrap();
        let markdown = project_markdown(
            &entity,
            br#"{"entities":{"Q42":{"id":"Q42","labels":{"en":{"value":"Douglas Adams"}},"descriptions":{"en":{"value":"English writer"}},"claims":{"P31":[{"mainsnak":{"snaktype":"value","datavalue":{"type":"wikibase-entityid","value":{"id":"Q5"}}},"rank":"normal"}]}}}}"#,
        )
        .unwrap();
        assert!(markdown.contains("Douglas Adams (Q42)"));
        assert!(markdown.contains("English writer"));
        assert!(markdown.contains("Q5"));
    }
}
