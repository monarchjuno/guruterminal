use super::*;

const OPENDART_CORP_CODE_PATH: &[&str] = &["api", "corpCode.xml"];
const ENTITY_CACHE_TTL: ChronoDuration = ChronoDuration::hours(24);

#[derive(Clone, Debug)]
pub(super) struct EntityRecord {
    pub name: String,
    pub symbol: Option<String>,
    pub cik: Option<String>,
    pub corp_code: Option<String>,
    pub currency: Option<String>,
    pub provider: &'static str,
}

#[derive(Default)]
pub(super) struct EntityDirectoryCache {
    dart: Option<CachedDirectory>,
}

struct CachedDirectory {
    fetched_at: DateTime<Utc>,
    entries: Vec<EntityRecord>,
}

#[derive(Clone, Copy)]
enum MatchGrade {
    TickerExact,
    NameExact,
    NameContains,
}

impl MatchGrade {
    fn as_str(self) -> &'static str {
        match self {
            Self::TickerExact => "ticker_exact",
            Self::NameExact => "name_exact",
            Self::NameContains => "name_contains",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::TickerExact => 0,
            Self::NameExact => 1,
            Self::NameContains => 2,
        }
    }
}

pub(super) fn parse_opendart_corp_code(
    bytes: &[u8],
) -> Result<Vec<EntityRecord>, FinanceDataError> {
    let xml = if bytes.starts_with(b"PK") {
        extract_corp_code_xml(bytes)?
    } else {
        String::from_utf8(bytes.to_vec()).map_err(|_| FinanceDataError::InvalidResponse)?
    };
    let mut entries = Vec::new();
    for block in xml.split("<list>").skip(1) {
        let Some(end) = block.find("</list>") else {
            continue;
        };
        let block = &block[..end];
        let corp_code = xml_tag(block, "corp_code");
        let name = xml_tag(block, "corp_name");
        let stock_code = xml_tag(block, "stock_code");
        if corp_code.len() != 8 || name.is_empty() {
            continue;
        }
        entries.push(EntityRecord {
            name,
            symbol: (!stock_code.is_empty()).then_some(stock_code),
            cik: None,
            corp_code: Some(corp_code),
            currency: Some("KRW".into()),
            provider: OPENDART_SOURCE_ID,
        });
    }
    Ok(entries)
}

fn extract_corp_code_xml(bytes: &[u8]) -> Result<String, FinanceDataError> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| FinanceDataError::InvalidResponse)?;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|_| FinanceDataError::InvalidResponse)?;
        if !file.name().to_ascii_lowercase().ends_with(".xml") {
            continue;
        }
        let mut xml = String::new();
        file.read_to_string(&mut xml)
            .map_err(|_| FinanceDataError::InvalidResponse)?;
        if xml.len() > MAX_DECOMPRESSED_DOCUMENT_BYTES {
            return Err(FinanceDataError::InvalidResponse);
        }
        return Ok(xml);
    }
    Err(FinanceDataError::InvalidResponse)
}

fn xml_tag(block: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    block
        .split_once(&open)
        .and_then(|(_, rest)| rest.split_once(&close))
        .map(|(value, _)| value.trim().to_owned())
        .unwrap_or_default()
}

pub(super) fn match_entities<'a>(
    query: &str,
    entries: impl IntoIterator<Item = &'a EntityRecord>,
    limit: usize,
) -> Vec<Value> {
    let needle = normalize_query(query);
    if needle.is_empty() {
        return Vec::new();
    }
    let mut scored = Vec::new();
    for entry in entries {
        let symbol = entry
            .symbol
            .as_deref()
            .map(normalize_query)
            .unwrap_or_default();
        let name = normalize_query(&entry.name);
        let grade = if !symbol.is_empty() && symbol == needle {
            Some(MatchGrade::TickerExact)
        } else if name == needle {
            Some(MatchGrade::NameExact)
        } else if name.contains(&needle) || symbol.contains(&needle) {
            Some(MatchGrade::NameContains)
        } else {
            None
        };
        if let Some(grade) = grade {
            scored.push((grade, entry));
        }
    }
    scored.sort_by_key(|(grade, entry)| (grade.rank(), entry.name.clone()));
    scored.truncate(limit);
    scored
        .into_iter()
        .map(|(grade, entry)| {
            json!({
                "name": entry.name,
                "symbol": entry.symbol,
                "cik": entry.cik,
                "corp_code": entry.corp_code,
                "exchange": Value::Null,
                "currency": entry.currency,
                "provider": entry.provider,
                "match_grade": grade.as_str(),
                "usage": "discovery"
            })
        })
        .collect()
}

fn normalize_query(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_uppercase)
        .collect()
}

impl FinanceDataService {
    pub async fn resolve_entity(
        &self,
        query: &str,
        limit: usize,
        dart_key: Option<&str>,
    ) -> Result<Value, FinanceDataError> {
        let mut matches = Vec::new();
        if let Some(key) = dart_key {
            let entries = self.opendart_entity_directory(key).await?;
            matches.extend(match_entities(query, &entries, limit));
        }
        matches.truncate(limit);
        Ok(json!({
            "kind": "finance_discovery",
            "usage": "discovery",
            "query": query,
            "matches": matches
        }))
    }

    async fn opendart_entity_directory(
        &self,
        credential: &str,
    ) -> Result<Vec<EntityRecord>, FinanceDataError> {
        if let Some(entries) = self.cached_directory(|cache| cache.dart.as_ref()).await {
            return Ok(entries);
        }
        validate_api_key(credential, OPENDART_SOURCE_ID)?;
        let mut public_url = fixed_url(OPENDART_API_ROOT, OPENDART_CORP_CODE_PATH)?;
        public_url
            .query_pairs_mut()
            .append_pair("crtfc_key", credential);
        let raw = self
            .fetch_provider(
                self.client.get(public_url.clone()),
                OPENDART_SOURCE_ID,
                public_url.to_string(),
                MAX_LARGE_PROVIDER_BYTES,
            )
            .await?;
        let entries = parse_opendart_corp_code(&raw.bytes)?;
        self.store_directory(|cache| &mut cache.dart, entries.clone())
            .await;
        Ok(entries)
    }

    async fn cached_directory(
        &self,
        select: impl Fn(&EntityDirectoryCache) -> Option<&CachedDirectory>,
    ) -> Option<Vec<EntityRecord>> {
        let cache = self.entity_directories.lock().await;
        let directory = select(&cache)?;
        if Utc::now() - directory.fetched_at > ENTITY_CACHE_TTL {
            return None;
        }
        Some(directory.entries.clone())
    }

    async fn store_directory(
        &self,
        select: impl FnOnce(&mut EntityDirectoryCache) -> &mut Option<CachedDirectory>,
        entries: Vec<EntityRecord>,
    ) {
        let mut cache = self.entity_directories.lock().await;
        *select(&mut cache) = Some(CachedDirectory {
            fetched_at: Utc::now(),
            entries,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dart_codes_match_exact_symbols() {
        let dart = parse_opendart_corp_code(
            r#"<result><list><corp_code>00126380</corp_code><corp_name>삼성전자</corp_name><stock_code>005930</stock_code></list></result>"#
                .as_bytes(),
        )
        .unwrap();
        let samsung = match_entities("005930", &dart, 5);
        assert_eq!(samsung[0]["corp_code"], "00126380");
        assert_eq!(samsung[0]["name"], "삼성전자");
        assert_eq!(samsung[0]["usage"], "discovery");
    }
}
