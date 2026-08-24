use super::*;

pub(super) fn parse_date(
    value: &str,
    message: &'static str,
) -> Result<NaiveDate, FinanceDataError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| FinanceDataError::InvalidQuery(message))
}

pub(super) fn parse_compact_date(value: &str) -> Result<NaiveDate, FinanceDataError> {
    NaiveDate::parse_from_str(value, "%Y%m%d").map_err(|_| FinanceDataError::InvalidResponse)
}

pub(super) fn validate_date_range(
    start: &str,
    end: &str,
    maximum_days: i64,
) -> Result<(NaiveDate, NaiveDate), FinanceDataError> {
    let start = parse_date(start, "start date is invalid")?;
    let end = parse_date(end, "end date is invalid")?;
    if start > end
        || start.year() < 1900
        || end > Utc::now().date_naive()
        || (end - start).num_days() > maximum_days
    {
        return Err(FinanceDataError::InvalidQuery(
            "date range is invalid or exceeds the bounded span",
        ));
    }
    Ok((start, end))
}

pub(super) fn validate_fiscal_year(year: i32) -> Result<(), FinanceDataError> {
    if year < 1994 || year > Utc::now().year() + 1 {
        return Err(FinanceDataError::InvalidQuery("fiscal year is invalid"));
    }
    Ok(())
}

pub(super) fn validate_digits(
    value: &str,
    length: usize,
    message: &'static str,
) -> Result<String, FinanceDataError> {
    if value.len() != length || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(FinanceDataError::InvalidQuery(message));
    }
    Ok(value.to_owned())
}

pub(super) fn dart_report_code(value: &str) -> Result<&'static str, FinanceDataError> {
    match value {
        "annual" => Ok("11011"),
        "q1" => Ok("11013"),
        "half_year" => Ok("11012"),
        "q3" => Ok("11014"),
        _ => Err(FinanceDataError::InvalidQuery(
            "report_period is unsupported",
        )),
    }
}

pub(super) fn validate_dart_forms(forms: &[String]) -> Result<(), FinanceDataError> {
    if forms.len() > 1
        || forms
            .iter()
            .any(|form| form.len() != 1 || !matches!(form.as_bytes()[0], b'A'..=b'J'))
    {
        return Err(FinanceDataError::InvalidQuery(
            "OpenDART accepts at most one disclosure type A through J",
        ));
    }
    Ok(())
}

pub(super) fn canonical_decimal(value: &Value) -> Option<String> {
    let raw = match value {
        Value::String(text) => text.replace(',', ""),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    let raw = raw.trim();
    if raw.is_empty() || raw == "." {
        return None;
    }
    let (negative, unsigned) = raw
        .strip_prefix('-')
        .map_or((false, raw), |rest| (true, rest));
    let mut pieces = unsigned.split('.');
    let integer = pieces.next()?;
    let fraction = pieces.next();
    if pieces.next().is_some()
        || integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.is_some_and(|part| !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let integer = integer.trim_start_matches('0');
    let integer = if integer.is_empty() { "0" } else { integer };
    let fraction = fraction.unwrap_or_default().trim_end_matches('0');
    let mut normalized = if fraction.is_empty() {
        integer.to_owned()
    } else {
        format!("{integer}.{fraction}")
    };
    if negative && normalized != "0" {
        normalized.insert(0, '-');
    }
    Some(normalized)
}

pub(super) fn normalized_sha256(value: &Value) -> Result<String, FinanceDataError> {
    let bytes = serde_json::to_vec(&canonicalize_json(value))
        .map_err(|_| FinanceDataError::InvalidResponse)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub(super) fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonicalize_json).collect()),
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut canonical = Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonicalize_json(&object[key]));
            }
            Value::Object(canonical)
        }
        scalar => scalar.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finance_result(
    tool: &str,
    operation: &str,
    source_id: &str,
    query: Value,
    data: Value,
    raw: &RawProviderResponse,
    data_authority: &str,
    provider: &str,
    official_source: bool,
    source_class: &str,
    revision_semantics: &str,
    quality_status: &str,
    checks: Value,
    warnings: Vec<String>,
    returned: usize,
    available: usize,
    truncated: bool,
) -> Result<Value, FinanceDataError> {
    Ok(json!({
        "schema_version": "guruterminal-finance-result/1",
        "tool": tool,
        "operation": operation,
        "source_id": source_id,
        "query": query,
        "data": data,
        "provenance": {
            "data_authority": data_authority,
            "provider": provider,
            "official_source": official_source,
            "source_url": raw.source_url,
            "source_urls": [raw.source_url],
            "retrieved_at": raw.retrieved_at,
            "revision_semantics": revision_semantics
        },
        "quality": {
            "source_class": source_class,
            "status": quality_status,
            "checks": checks,
            "warnings": warnings
        },
        "warnings": warnings,
        "page": {
            "returned": returned,
            "available": available,
            "truncated": truncated
        }
    }))
}

pub(super) fn standard_checks() -> Value {
    json!([
        {"code": "provider_success", "status": "pass"},
        {"code": "response_complete", "status": "pass"},
        {"code": "schema_valid", "status": "pass"}
    ])
}

pub(super) fn validate_ohlcv_strings(
    open: &str,
    high: &str,
    low: &str,
    close: &str,
    volume: &str,
) -> Result<(), FinanceDataError> {
    let parse = |value: &str| {
        value
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .ok_or(FinanceDataError::InvalidResponse)
    };
    let open = parse(open)?;
    let high = parse(high)?;
    let low = parse(low)?;
    let close = parse(close)?;
    let volume = parse(volume)?;
    if low > open || low > close || low > high || high < open || high < close || volume < 0.0 {
        return Err(FinanceDataError::InvalidResponse);
    }
    Ok(())
}

pub(super) fn extract_text(bytes: &[u8]) -> (String, bool) {
    let decoded = String::from_utf8_lossy(bytes);
    let document = Html::parse_document(&decoded);
    let normalized = document
        .root_element()
        .text()
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    truncate_chars(normalized, MAX_DOCUMENT_TEXT_CHARS)
}

pub(super) fn truncate_chars(text: String, maximum: usize) -> (String, bool) {
    if text.chars().count() <= maximum {
        return (text, false);
    }
    (text.chars().take(maximum).collect(), true)
}

pub(super) fn detect_krx_error(bytes: &[u8]) -> Result<Value, FinanceDataError> {
    let root: Value =
        serde_json::from_slice(bytes).map_err(|_| FinanceDataError::InvalidResponse)?;
    match root.pointer("/result/code").and_then(Value::as_str) {
        Some("000") => Ok(root),
        Some(_) => Err(FinanceDataError::CredentialRejected(KRX_SOURCE_ID)),
        None if root.get("OutBlock_1").is_some_and(Value::is_array) => Ok(root),
        None => Err(FinanceDataError::InvalidResponse),
    }
}

pub(super) fn validate_dart_status_allow_no_data(bytes: &[u8]) -> Result<Value, FinanceDataError> {
    let root: Value =
        serde_json::from_slice(bytes).map_err(|_| FinanceDataError::InvalidResponse)?;
    match root.get("status").and_then(Value::as_str) {
        Some("000" | "013") => Ok(root),
        Some("010" | "011" | "012") => {
            Err(FinanceDataError::CredentialRejected(OPENDART_SOURCE_ID))
        }
        Some("020") => Err(FinanceDataError::RateLimited(OPENDART_SOURCE_ID)),
        Some(_) => Err(FinanceDataError::Provider(
            "OpenDART returned a provider status error".to_owned(),
        )),
        None => Err(FinanceDataError::InvalidResponse),
    }
}

pub(super) fn validate_dart_status(bytes: &[u8]) -> Result<Value, FinanceDataError> {
    let root = validate_dart_status_allow_no_data(bytes)?;
    if root.get("status").and_then(Value::as_str) == Some("013") {
        return Err(FinanceDataError::NoData(OPENDART_SOURCE_ID));
    }
    Ok(root)
}
