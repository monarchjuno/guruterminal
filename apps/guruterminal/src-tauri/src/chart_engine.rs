use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::chat_artifacts::ChatArtifactError;

pub const CHART_SCHEMA: &str = "guruterminal-chart/2";
const MAX_COLUMNS: usize = 32;
const MAX_ROWS: usize = 10_000;
const MAX_CELL_TEXT_BYTES: usize = 8 * 1024;
const MAX_DATASET_BYTES: usize = 16 * 1024 * 1024;
const MAX_COLUMN_LABEL_BYTES: usize = 160;
const MAX_JSON_POINTER_BYTES: usize = 2 * 1024;
const MAX_LINEAGE_RESULTS: usize = 32;
const MAX_LINEAGE_WARNINGS: usize = 32;
const MAX_LINEAGE_WARNING_BYTES: usize = 2 * 1024;
const MAX_STUDIES: usize = 12;
const MAX_DRAWINGS: usize = 32;
const MAX_DRAWING_LABEL_BYTES: usize = 80;
const MAX_CHART_INTERVAL_SPAN: u16 = 9_999;
const UNIX_SECONDS_CUTOFF: f64 = 10_000_000_000.0;
const JAVASCRIPT_TIMESTAMP_LIMIT_MS: f64 = 8_640_000_000_000_000.0;

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_:/".contains(&byte) || byte == b'.')
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum && !value.contains('\0')
}

fn valid_chart_interval(value: &str) -> bool {
    ["wk", "mo", "m", "h", "d"].into_iter().any(|suffix| {
        value.strip_suffix(suffix).is_some_and(|span| {
            !span.is_empty()
                && !span.starts_with('0')
                && span.bytes().all(|byte| byte.is_ascii_digit())
                && span
                    .parse::<u16>()
                    .is_ok_and(|span| (1..=MAX_CHART_INTERVAL_SPAN).contains(&span))
        })
    })
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartColumnKind {
    String,
    Number,
    Boolean,
    Date,
    Datetime,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartColumn {
    pub id: String,
    pub label: String,
    pub kind: ChartColumnKind,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartResultReceipt {
    pub result_ref: String,
    pub runtime_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub provider: Option<String>,
    pub request_digest: String,
    pub response_digest: String,
    pub retrieved_at: String,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub upstream_result_refs: Vec<String>,
}

impl ChartResultReceipt {
    fn validate(&self) -> Result<(), ChatArtifactError> {
        if !valid_identifier(&self.result_ref)
            || !valid_identifier(&self.runtime_id)
            || !valid_identifier(&self.tool_name)
            || self
                .provider
                .as_deref()
                .is_some_and(|provider| !valid_identifier(provider))
            || !valid_digest(&self.request_digest)
            || !valid_digest(&self.response_digest)
            || !valid_text(&self.retrieved_at, 80)
            || self.warnings.len() > MAX_LINEAGE_WARNINGS
            || self
                .warnings
                .iter()
                .any(|warning| !valid_text(warning, MAX_LINEAGE_WARNING_BYTES))
            || self.upstream_result_refs.len() > MAX_LINEAGE_RESULTS
            || self
                .upstream_result_refs
                .iter()
                .any(|result_ref| !valid_identifier(result_ref))
        {
            return Err(ChatArtifactError::Invalid("chart result receipt"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartResultColumnSelection {
    pub id: String,
    pub label: String,
    pub kind: ChartColumnKind,
    pub pointer: String,
}

impl ChartResultColumnSelection {
    fn validate(&self) -> Result<(), ChatArtifactError> {
        validate_column(&ChartColumn {
            id: self.id.clone(),
            label: self.label.clone(),
            kind: self.kind.clone(),
        })?;
        validate_json_pointer(&self.pointer)
    }

    fn column(&self) -> ChartColumn {
        ChartColumn {
            id: self.id.clone(),
            label: self.label.clone(),
            kind: self.kind.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChartDatasetLineage {
    FromResult {
        receipt: Box<ChartResultReceipt>,
        rows_pointer: String,
        columns: Vec<ChartResultColumnSelection>,
    },
    AgentAuthored {
        #[serde(default)]
        upstream_receipts: Vec<ChartResultReceipt>,
    },
}

impl ChartDatasetLineage {
    fn validate(&self) -> Result<(), ChatArtifactError> {
        match self {
            Self::FromResult {
                receipt,
                rows_pointer,
                columns,
            } => {
                receipt.validate()?;
                validate_json_pointer(rows_pointer)?;
                if columns.is_empty() || columns.len() > MAX_COLUMNS {
                    return Err(ChatArtifactError::Invalid("chart result selection"));
                }
                let mut ids = BTreeSet::new();
                for column in columns {
                    column.validate()?;
                    if !ids.insert(column.id.as_str()) {
                        return Err(ChatArtifactError::Invalid("chart result selection"));
                    }
                }
            }
            Self::AgentAuthored { upstream_receipts } => {
                if upstream_receipts.len() > MAX_LINEAGE_RESULTS {
                    return Err(ChatArtifactError::Invalid("chart dataset lineage"));
                }
                let mut result_refs = BTreeSet::new();
                for receipt in upstream_receipts {
                    receipt.validate()?;
                    if !result_refs.insert(receipt.result_ref.as_str()) {
                        return Err(ChatArtifactError::Invalid("chart dataset lineage"));
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartDataset {
    pub id: String,
    pub columns: Vec<ChartColumn>,
    pub rows: Vec<Vec<Value>>,
    pub lineage: ChartDatasetLineage,
    pub digest: String,
}

impl ChartDataset {
    pub fn new(
        id: String,
        columns: Vec<ChartColumn>,
        rows: Vec<Vec<Value>>,
        lineage: ChartDatasetLineage,
    ) -> Result<Self, ChatArtifactError> {
        let digest = dataset_digest(&columns, &rows, &lineage)?;
        let dataset = Self {
            id,
            columns,
            rows,
            lineage,
            digest,
        };
        dataset.validate()?;
        Ok(dataset)
    }

    pub fn validate(&self) -> Result<(), ChatArtifactError> {
        if !valid_identifier(&self.id)
            || self.columns.is_empty()
            || self.columns.len() > MAX_COLUMNS
            || self.rows.is_empty()
            || self.rows.len() > MAX_ROWS
        {
            return Err(ChatArtifactError::Invalid("chart dataset shape"));
        }
        let mut names = BTreeSet::new();
        for column in &self.columns {
            validate_column(column)?;
            if !names.insert(column.id.as_str()) {
                return Err(ChatArtifactError::Invalid("chart dataset column"));
            }
        }
        for row in &self.rows {
            if row.len() != self.columns.len()
                || row
                    .iter()
                    .zip(&self.columns)
                    .any(|(cell, column)| !valid_typed_cell(cell, &column.kind))
            {
                return Err(ChatArtifactError::Invalid("chart dataset row"));
            }
        }
        self.lineage.validate()?;
        let encoded = serde_json::to_vec(&(&self.columns, &self.rows, &self.lineage))
            .map_err(|_| ChatArtifactError::Digest)?;
        if encoded.len() > MAX_DATASET_BYTES || self.digest != hex::encode(Sha256::digest(&encoded))
        {
            return Err(ChatArtifactError::Invalid("chart dataset digest"));
        }
        Ok(())
    }

    pub fn summary(&self) -> ChartDatasetSummary {
        ChartDatasetSummary {
            dataset_id: self.id.clone(),
            columns: self.columns.clone(),
            row_count: self.rows.len(),
            lineage: self.lineage.clone(),
            digest: self.digest.clone(),
        }
    }
}

fn validate_column(column: &ChartColumn) -> Result<(), ChatArtifactError> {
    if !valid_identifier(&column.id) || !valid_text(&column.label, MAX_COLUMN_LABEL_BYTES) {
        return Err(ChatArtifactError::Invalid("chart dataset column"));
    }
    Ok(())
}

fn valid_typed_cell(value: &Value, kind: &ChartColumnKind) -> bool {
    if value.is_null() {
        return true;
    }
    if !valid_cell(value) {
        return false;
    }
    match kind {
        ChartColumnKind::String => value.is_string(),
        ChartColumnKind::Number => chart_number(value).is_some(),
        ChartColumnKind::Boolean => value.is_boolean(),
        ChartColumnKind::Date => value
            .as_str()
            .is_some_and(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()),
        ChartColumnKind::Datetime => chart_timestamp(value).is_some(),
    }
}

fn valid_cell(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
        Value::String(value) => value.len() <= MAX_CELL_TEXT_BYTES && !value.contains('\0'),
        Value::Array(_) | Value::Object(_) => false,
    }
}

fn dataset_digest(
    columns: &[ChartColumn],
    rows: &[Vec<Value>],
    lineage: &ChartDatasetLineage,
) -> Result<String, ChatArtifactError> {
    let bytes =
        serde_json::to_vec(&(columns, rows, lineage)).map_err(|_| ChatArtifactError::Digest)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_json_pointer(pointer: &str) -> Result<(), ChatArtifactError> {
    if !crate::json_pointer::valid_json_pointer(pointer, MAX_JSON_POINTER_BYTES, true) {
        return Err(ChatArtifactError::Invalid("chart JSON pointer"));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ChartDatasetSummary {
    pub dataset_id: String,
    pub columns: Vec<ChartColumn>,
    pub row_count: usize,
    pub lineage: ChartDatasetLineage,
    pub digest: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartDocument {
    pub dataset_id: String,
    pub dataset_digest: String,
    pub view: ChartView,
    #[serde(default)]
    pub studies: Vec<ChartStudy>,
    #[serde(default)]
    pub drawings: Vec<ChartDrawing>,
    #[serde(default)]
    pub note: Option<String>,
}

impl ChartDocument {
    pub fn validate(&self) -> Result<(), ChatArtifactError> {
        if !valid_identifier(&self.dataset_id)
            || self.dataset_digest.len() != 64
            || !self
                .dataset_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.studies.len() > MAX_STUDIES
            || self.drawings.len() > MAX_DRAWINGS
            || self
                .note
                .as_deref()
                .is_some_and(|value| value.len() > 2_000 || value.contains('\0'))
        {
            return Err(ChatArtifactError::Invalid("chart document"));
        }
        self.view.validate()?;
        if matches!(self.view, ChartView::Analytic { .. })
            && (!self.studies.is_empty() || !self.drawings.is_empty())
        {
            return Err(ChatArtifactError::Invalid(
                "analytic chart study or drawing",
            ));
        }
        let mut module_ids = BTreeSet::new();
        for study in &self.studies {
            study.validate()?;
            if !module_ids.insert(study.module_id) {
                return Err(ChatArtifactError::Invalid("duplicate chart study"));
            }
        }
        for drawing in &self.drawings {
            drawing.validate()?;
        }
        Ok(())
    }

    pub fn validate_dataset(&self, dataset: &ChartDataset) -> Result<(), ChatArtifactError> {
        dataset.validate()?;
        if dataset.id != self.dataset_id || dataset.digest != self.dataset_digest {
            return Err(ChatArtifactError::Invalid("chart dataset binding"));
        }
        let fields = dataset
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| (column.id.as_str(), index))
            .collect::<BTreeMap<_, _>>();
        if self
            .view
            .referenced_fields()
            .iter()
            .any(|field| !fields.contains_key(field))
        {
            return Err(ChatArtifactError::Invalid("chart dataset field binding"));
        }
        if let ChartView::Financial {
            time,
            open,
            high,
            low,
            close,
            volume,
            turnover,
            ..
        } = &self.view
        {
            if self
                .studies
                .iter()
                .any(|study| study.module_id.requires_volume())
                && volume.is_none()
            {
                return Err(ChatArtifactError::Invalid(
                    "financial chart study requires volume",
                ));
            }
            if self
                .studies
                .iter()
                .any(|study| study.module_id.requires_turnover())
                && turnover.is_none()
            {
                return Err(ChatArtifactError::Invalid(
                    "financial chart study requires turnover",
                ));
            }
            validate_financial_rows(
                dataset,
                FinancialFieldIndices {
                    time: fields[time.as_str()],
                    open: fields[open.as_str()],
                    high: fields[high.as_str()],
                    low: fields[low.as_str()],
                    close: fields[close.as_str()],
                    volume: volume.as_deref().map(|field| fields[field]),
                    turnover: turnover.as_deref().map(|field| fields[field]),
                },
            )?;
            validate_financial_drawing_bounds(dataset, fields[time.as_str()], &self.drawings)?;
        }
        Ok(())
    }
}

fn validate_financial_drawing_bounds(
    dataset: &ChartDataset,
    time: usize,
    drawings: &[ChartDrawing],
) -> Result<(), ChatArtifactError> {
    let first = chart_timestamp(&dataset.rows[0][time])
        .ok_or(ChatArtifactError::Invalid("financial chart timestamp"))?;
    let last = chart_timestamp(&dataset.rows[dataset.rows.len() - 1][time])
        .ok_or(ChatArtifactError::Invalid("financial chart timestamp"))?;
    if drawings
        .iter()
        .flat_map(|drawing| &drawing.points)
        .any(|point| {
            chart_timestamp(&point.timestamp)
                .is_none_or(|timestamp| timestamp < first || timestamp > last)
        })
    {
        return Err(ChatArtifactError::Invalid("financial chart drawing range"));
    }
    Ok(())
}

struct FinancialFieldIndices {
    time: usize,
    open: usize,
    high: usize,
    low: usize,
    close: usize,
    volume: Option<usize>,
    turnover: Option<usize>,
}

fn validate_financial_rows(
    dataset: &ChartDataset,
    fields: FinancialFieldIndices,
) -> Result<(), ChatArtifactError> {
    let mut previous_time = None;
    for row in &dataset.rows {
        let timestamp = chart_timestamp(&row[fields.time])
            .ok_or(ChatArtifactError::Invalid("financial chart timestamp"))?;
        if previous_time.is_some_and(|previous| timestamp <= previous) {
            return Err(ChatArtifactError::Invalid(
                "financial chart timestamp order",
            ));
        }
        previous_time = Some(timestamp);
        let open = chart_number(&row[fields.open])
            .ok_or(ChatArtifactError::Invalid("financial chart OHLC"))?;
        let high = chart_number(&row[fields.high])
            .ok_or(ChatArtifactError::Invalid("financial chart OHLC"))?;
        let low = chart_number(&row[fields.low])
            .ok_or(ChatArtifactError::Invalid("financial chart OHLC"))?;
        let close = chart_number(&row[fields.close])
            .ok_or(ChatArtifactError::Invalid("financial chart OHLC"))?;
        if high < open.max(close) || low > open.min(close) || high < low {
            return Err(ChatArtifactError::Invalid("financial chart OHLC range"));
        }
        if fields
            .volume
            .is_some_and(|index| chart_number(&row[index]).is_none_or(|value| value < 0.0))
        {
            return Err(ChatArtifactError::Invalid("financial chart volume"));
        }
        if fields
            .turnover
            .is_some_and(|index| chart_number(&row[index]).is_none_or(|value| value < 0.0))
        {
            return Err(ChatArtifactError::Invalid("financial chart turnover"));
        }
    }
    Ok(())
}

fn chart_number(value: &Value) -> Option<f64> {
    let value = match value {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    value.is_finite().then_some(value)
}

fn chart_timestamp(value: &Value) -> Option<f64> {
    match value {
        // Numeric timestamps are unambiguous only for the Unix era. Values
        // below the cutoff are seconds; larger values are milliseconds.
        // Pre-1970 dates remain available through explicit ISO strings.
        Value::Number(_) => chart_number(value).and_then(|timestamp| {
            if timestamp < 0.0 {
                return None;
            }
            let timestamp_ms = if timestamp < UNIX_SECONDS_CUTOFF {
                timestamp * 1_000.0
            } else {
                timestamp
            };
            (timestamp_ms <= JAVASCRIPT_TIMESTAMP_LIMIT_MS).then_some(timestamp_ms)
        }),
        Value::String(value) => chrono::DateTime::parse_from_rfc3339(value)
            .map(|timestamp| timestamp.timestamp_millis() as f64)
            .or_else(|_| {
                chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").map(|date| {
                    date.and_hms_opt(0, 0, 0)
                        .unwrap()
                        .and_utc()
                        .timestamp_millis() as f64
                })
            })
            .ok()
            .filter(|timestamp| timestamp.abs() <= JAVASCRIPT_TIMESTAMP_LIMIT_MS),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChartView {
    Financial {
        symbol: String,
        interval: String,
        time: String,
        open: String,
        high: String,
        low: String,
        close: String,
        #[serde(default)]
        volume: Option<String>,
        #[serde(default)]
        turnover: Option<String>,
        #[serde(default)]
        price_precision: Option<u8>,
    },
    Analytic {
        chart_type: AnalyticChartType,
        x: String,
        y: Vec<String>,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        semantic_types: BTreeMap<String, String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        subtitle: Option<String>,
    },
}

impl ChartView {
    fn validate(&self) -> Result<(), ChatArtifactError> {
        match self {
            Self::Financial {
                symbol,
                interval,
                time,
                open,
                high,
                low,
                close,
                volume,
                turnover,
                price_precision,
            } => {
                if !valid_text(symbol, 120)
                    || !valid_chart_interval(interval)
                    || [time, open, high, low, close]
                        .iter()
                        .any(|field| !valid_identifier(field))
                    || volume
                        .as_deref()
                        .is_some_and(|field| !valid_identifier(field))
                    || turnover
                        .as_deref()
                        .is_some_and(|field| !valid_identifier(field))
                    || price_precision.is_some_and(|value| value > 12)
                {
                    return Err(ChatArtifactError::Invalid("financial chart view"));
                }
            }
            Self::Analytic {
                x,
                y,
                color,
                semantic_types,
                title,
                subtitle,
                ..
            } => {
                let mut referenced_fields = BTreeSet::from([x.as_str()]);
                referenced_fields.extend(y.iter().map(String::as_str));
                referenced_fields.extend(color.as_deref());
                if !valid_identifier(x)
                    || y.is_empty()
                    || y.len() > 8
                    || y.iter().any(|field| !valid_identifier(field))
                    || (color.is_some() && y.len() > 1)
                    || referenced_fields.len() != 1 + y.len() + usize::from(color.is_some())
                    || color
                        .as_deref()
                        .is_some_and(|field| !valid_identifier(field))
                    || semantic_types.len() > MAX_COLUMNS
                    || semantic_types.iter().any(|(field, semantic)| {
                        !referenced_fields.contains(field.as_str()) || !valid_text(semantic, 64)
                    })
                    || title
                        .as_deref()
                        .is_some_and(|value| !valid_text(value, 200))
                    || subtitle
                        .as_deref()
                        .is_some_and(|value| !valid_text(value, 400))
                {
                    return Err(ChatArtifactError::Invalid("analytic chart view"));
                }
            }
        }
        Ok(())
    }

    pub fn referenced_fields(&self) -> Vec<&str> {
        match self {
            Self::Financial {
                time,
                open,
                high,
                low,
                close,
                volume,
                turnover,
                ..
            } => {
                let mut fields = vec![time.as_str(), open, high, low, close];
                fields.extend(volume.as_deref());
                fields.extend(turnover.as_deref());
                fields
            }
            Self::Analytic { x, y, color, .. } => {
                let mut fields = vec![x.as_str()];
                fields.extend(y.iter().map(String::as_str));
                fields.extend(color.as_deref());
                fields
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticChartType {
    Line,
    Area,
    Bar,
    Scatter,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartDrawing {
    pub kind: ChartDrawingKind,
    pub points: Vec<ChartDrawingPoint>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub line_width: Option<u8>,
    #[serde(default)]
    pub line_style: Option<ChartDrawingLineStyle>,
    #[serde(default)]
    pub label: Option<String>,
}

impl ChartDrawing {
    fn validate(&self) -> Result<(), ChatArtifactError> {
        let label_ok = match (&self.kind, self.label.as_deref()) {
            (ChartDrawingKind::Annotation, None) => false,
            (_, None) => true,
            (_, Some(label)) => valid_drawing_label(label),
        };
        if self.points.len() != self.kind.point_count()
            || !label_ok
            || self
                .color
                .as_deref()
                .is_some_and(|color| !valid_chart_color(color))
            || self
                .line_width
                .is_some_and(|line_width| !(1..=8).contains(&line_width))
            || self.points.iter().any(|point| {
                chart_timestamp(&point.timestamp).is_none() || !point.value.is_finite()
            })
        {
            return Err(ChatArtifactError::Invalid("financial chart drawing"));
        }
        Ok(())
    }
}

fn valid_drawing_label(label: &str) -> bool {
    !label.trim().is_empty()
        && label.len() <= MAX_DRAWING_LABEL_BYTES
        && !label.contains('\0')
        && !label.contains('\n')
        && !label.contains('\r')
}

fn valid_chart_color(color: &str) -> bool {
    matches!(color.len(), 7 | 9)
        && color.starts_with('#')
        && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartDrawingPoint {
    pub timestamp: Value,
    pub value: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartDrawingKind {
    Segment,
    Ray,
    Line,
    HorizontalLine,
    VerticalLine,
    PriceLine,
    Fibonacci,
    HorizontalSegment,
    HorizontalRay,
    VerticalSegment,
    VerticalRay,
    ParallelLine,
    PriceChannel,
    Annotation,
    Rectangle,
    Arrow,
    Measure,
    FibonacciExtension,
    LongPosition,
    ShortPosition,
}

impl ChartDrawingKind {
    fn point_count(self) -> usize {
        match self {
            Self::HorizontalLine | Self::VerticalLine | Self::PriceLine | Self::Annotation => 1,
            Self::ParallelLine
            | Self::PriceChannel
            | Self::FibonacciExtension
            | Self::LongPosition
            | Self::ShortPosition => 3,
            _ => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartDrawingLineStyle {
    Solid,
    Dashed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartStudy {
    pub module_id: ChartStudyModule,
    #[serde(default)]
    pub calc_params: Vec<f64>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ChartStudyModule {
    Avp,
    Ao,
    Bias,
    Brar,
    Bbi,
    Vol,
    Ma,
    Ema,
    Sma,
    Boll,
    Cci,
    Cr,
    Dma,
    Dmi,
    Emv,
    Macd,
    Mtm,
    Obv,
    Pvt,
    Psy,
    Roc,
    Rsi,
    Kdj,
    Sar,
    Trix,
    Vr,
    Wr,
}

impl ChartStudyModule {
    fn requires_volume(self) -> bool {
        matches!(
            self,
            Self::Avp | Self::Emv | Self::Obv | Self::Pvt | Self::Vol | Self::Vr
        )
    }

    fn requires_turnover(self) -> bool {
        matches!(self, Self::Avp)
    }
}

impl ChartStudy {
    fn validate(&self) -> Result<(), ChatArtifactError> {
        let valid_params = self.calc_params.is_empty()
            || match self.module_id {
                ChartStudyModule::Vol
                | ChartStudyModule::Ma
                | ChartStudyModule::Ema
                | ChartStudyModule::Rsi
                | ChartStudyModule::Bias
                | ChartStudyModule::Wr => {
                    self.calc_params.len() <= 16
                        && self.calc_params.iter().all(|value| valid_period(*value))
                }
                ChartStudyModule::Avp | ChartStudyModule::Pvt => false,
                ChartStudyModule::Ao => {
                    valid_period_shape(&self.calc_params, 2)
                        && self.calc_params[0] < self.calc_params[1]
                }
                ChartStudyModule::Boll => {
                    self.calc_params.len() == 2
                        && valid_period(self.calc_params[0])
                        && valid_positive(self.calc_params[1])
                }
                ChartStudyModule::Sma => {
                    self.calc_params.len() == 2
                        && self.calc_params.iter().all(|value| valid_period(*value))
                        && self.calc_params[1] <= self.calc_params[0]
                }
                ChartStudyModule::Macd => {
                    valid_period_shape(&self.calc_params, 3)
                        && self.calc_params[0] < self.calc_params[1]
                }
                ChartStudyModule::Kdj => valid_period_shape(&self.calc_params, 3),
                ChartStudyModule::Brar | ChartStudyModule::Cci | ChartStudyModule::Obv => {
                    valid_period_shape(&self.calc_params, 1)
                }
                ChartStudyModule::Bbi => valid_period_shape(&self.calc_params, 4),
                ChartStudyModule::Cr => valid_period_shape(&self.calc_params, 5),
                ChartStudyModule::Dma => {
                    valid_period_shape(&self.calc_params, 3)
                        && self.calc_params[0] < self.calc_params[1]
                }
                ChartStudyModule::Dmi
                | ChartStudyModule::Emv
                | ChartStudyModule::Mtm
                | ChartStudyModule::Psy
                | ChartStudyModule::Roc
                | ChartStudyModule::Trix
                | ChartStudyModule::Vr => valid_period_shape(&self.calc_params, 2),
                ChartStudyModule::Sar => {
                    self.calc_params.len() == 3
                        && self.calc_params.iter().all(|value| valid_positive(*value))
                        && self.calc_params[0] <= self.calc_params[2]
                        && self.calc_params[1] <= self.calc_params[2]
                }
            };
        if !valid_params {
            return Err(ChatArtifactError::Invalid("chart study"));
        }
        Ok(())
    }
}

fn valid_period_shape(values: &[f64], length: usize) -> bool {
    values.len() == length && values.iter().all(|value| valid_period(*value))
}

fn valid_positive(value: f64) -> bool {
    value.is_finite() && value > 0.0 && value <= 10_000.0
}

fn valid_period(value: f64) -> bool {
    valid_positive(value) && value.fract() == 0.0
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum ChartDatasetInput {
    FromResult(ChartFromResultEnvelope),
    Inline(ChartInlineEnvelope),
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartFromResultEnvelope {
    pub from_result: ChartFromResultInput,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartInlineEnvelope {
    pub inline: ChartInlineInput,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartFromResultInput {
    pub result_ref: String,
    pub rows_pointer: String,
    pub columns: Vec<ChartResultColumnSelection>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartInlineInput {
    pub columns: Vec<ChartColumn>,
    pub rows: Vec<Vec<Value>>,
    #[serde(default)]
    pub upstream_result_refs: Vec<String>,
}

pub fn dataset_from_result_selection(
    id: String,
    input: ChartFromResultInput,
    payload: &Value,
    receipt: ChartResultReceipt,
) -> Result<ChartDataset, ChatArtifactError> {
    if input.result_ref != receipt.result_ref {
        return Err(ChatArtifactError::Invalid("chart result receipt binding"));
    }
    validate_json_pointer(&input.rows_pointer)?;
    if input.columns.is_empty() || input.columns.len() > MAX_COLUMNS {
        return Err(ChatArtifactError::Invalid("chart result selection"));
    }
    let selected_rows = payload
        .pointer(&input.rows_pointer)
        .and_then(Value::as_array)
        .ok_or(ChatArtifactError::Invalid("chart rows pointer"))?;
    if selected_rows.is_empty() || selected_rows.len() > MAX_ROWS {
        return Err(ChatArtifactError::Invalid("chart dataset shape"));
    }
    for column in &input.columns {
        column.validate()?;
    }
    let rows = selected_rows
        .iter()
        .map(|row| {
            input
                .columns
                .iter()
                .map(|column| {
                    row.pointer(&column.pointer)
                        .cloned()
                        .ok_or(ChatArtifactError::Invalid("chart column pointer"))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let columns = input
        .columns
        .iter()
        .map(ChartResultColumnSelection::column)
        .collect::<Vec<_>>();
    ChartDataset::new(
        id,
        columns,
        rows,
        ChartDatasetLineage::FromResult {
            receipt: Box::new(receipt),
            rows_pointer: input.rows_pointer,
            columns: input.columns,
        },
    )
}

pub fn dataset_from_inline(
    id: String,
    input: ChartInlineInput,
    upstream_receipts: Vec<ChartResultReceipt>,
) -> Result<ChartDataset, ChatArtifactError> {
    if input.upstream_result_refs.len() != upstream_receipts.len()
        || input
            .upstream_result_refs
            .iter()
            .zip(&upstream_receipts)
            .any(|(expected, receipt)| expected != &receipt.result_ref)
    {
        return Err(ChatArtifactError::Invalid("chart upstream receipt binding"));
    }
    ChartDataset::new(
        id,
        input.columns,
        input.rows,
        ChartDatasetLineage::AgentAuthored { upstream_receipts },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn receipt(result_ref: &str) -> ChartResultReceipt {
        ChartResultReceipt {
            result_ref: result_ref.into(),
            runtime_id: "openbb".into(),
            tool_name: "equity_price_historical".into(),
            provider: Some("yfinance".into()),
            request_digest: "a".repeat(64),
            response_digest: "b".repeat(64),
            retrieved_at: "2026-08-24T12:00:00Z".into(),
            warnings: vec!["Delayed market data".into()],
            upstream_result_refs: vec![],
        }
    }

    #[test]
    fn explicit_result_selection_becomes_a_digest_bound_dataset() {
        let dataset = dataset_from_result_selection(
            "dataset-1".into(),
            ChartFromResultInput {
                result_ref: "result:1".into(),
                rows_pointer: "/data/rows".into(),
                columns: vec![
                    ChartResultColumnSelection {
                        id: "period".into(),
                        label: "Period".into(),
                        kind: ChartColumnKind::Date,
                        pointer: "/period".into(),
                    },
                    ChartResultColumnSelection {
                        id: "close".into(),
                        label: "Close".into(),
                        kind: ChartColumnKind::Number,
                        pointer: "/close".into(),
                    },
                ],
            },
            &json!({"data":{"rows":[
                {"period":"2026-08-01","open":"10","high":"12","low":"9","close":"11","volume":"100"},
                {"period":"2026-08-02","open":"11","high":"13","low":"10","close":"12","volume":"110"}
            ]}}),
            receipt("result:1"),
        )
        .unwrap();
        assert_eq!(dataset.rows.len(), 2);
        assert_eq!(dataset.columns[0].id, "period");
        assert_eq!(dataset.columns[1].label, "Close");
        assert_eq!(dataset.rows[0], vec![json!("2026-08-01"), json!("11")]);
        assert!(matches!(
            &dataset.lineage,
            ChartDatasetLineage::FromResult { receipt, .. }
                if receipt.result_ref == "result:1"
        ));
        assert_eq!(dataset.digest.len(), 64);

        let mut tampered = dataset.clone();
        let ChartDatasetLineage::FromResult { receipt, .. } = &mut tampered.lineage else {
            unreachable!();
        };
        receipt.response_digest = "c".repeat(64);
        assert!(tampered.validate().is_err());
    }

    #[test]
    fn explicit_result_selection_rejects_missing_or_nested_cells() {
        for pointer in ["/missing", "/nested"] {
            assert!(dataset_from_result_selection(
                "dataset-1".into(),
                ChartFromResultInput {
                    result_ref: "result:1".into(),
                    rows_pointer: "/rows".into(),
                    columns: vec![ChartResultColumnSelection {
                        id: "value".into(),
                        label: "Value".into(),
                        kind: ChartColumnKind::Number,
                        pointer: pointer.into(),
                    }],
                },
                &json!({"rows":[{"value":10,"nested":{"value":10}}]}),
                receipt("result:1"),
            )
            .is_err());
        }
        for pointer in ["/bad~2", "/trailing~"] {
            assert!(
                dataset_from_result_selection(
                    "dataset-1".into(),
                    ChartFromResultInput {
                        result_ref: "result:1".into(),
                        rows_pointer: "/rows".into(),
                        columns: vec![ChartResultColumnSelection {
                            id: "value".into(),
                            label: "Value".into(),
                            kind: ChartColumnKind::Number,
                            pointer: pointer.into(),
                        }],
                    },
                    &json!({"rows":[{"value":10}]}),
                    receipt("result:1"),
                )
                .is_err(),
                "{pointer}"
            );
        }
    }

    #[test]
    fn inline_dataset_is_marked_agent_authored_and_binds_upstream_receipts() {
        let dataset = dataset_from_inline(
            "dataset-1".into(),
            ChartInlineInput {
                columns: vec![
                    ChartColumn {
                        id: "date".into(),
                        label: "Date".into(),
                        kind: ChartColumnKind::Date,
                    },
                    ChartColumn {
                        id: "return".into(),
                        label: "Return".into(),
                        kind: ChartColumnKind::Number,
                    },
                ],
                rows: vec![vec![json!("2026-08-01"), json!(0.15)]],
                upstream_result_refs: vec!["result:1".into()],
            },
            vec![receipt("result:1")],
        )
        .unwrap();
        assert!(matches!(
            &dataset.lineage,
            ChartDatasetLineage::AgentAuthored { upstream_receipts }
                if upstream_receipts[0].result_ref == "result:1"
        ));
        assert!(dataset.validate().is_ok());
    }

    #[test]
    fn chart_document_rejects_duplicate_or_non_finite_studies() {
        let study = ChartStudy {
            module_id: ChartStudyModule::Ema,
            calc_params: vec![20.0],
        };
        let mut document = ChartDocument {
            dataset_id: "dataset-1".into(),
            dataset_digest: "a".repeat(64),
            view: ChartView::Financial {
                symbol: "TEST".into(),
                interval: "1d".into(),
                time: "date".into(),
                open: "open".into(),
                high: "high".into(),
                low: "low".into(),
                close: "close".into(),
                volume: None,
                turnover: None,
                price_precision: None,
            },
            studies: vec![study.clone(), study],
            drawings: vec![],
            note: None,
        };
        assert!(document.validate().is_err());

        document.studies = vec![ChartStudy {
            module_id: ChartStudyModule::Ema,
            calc_params: vec![f64::INFINITY],
        }];
        assert!(document.validate().is_err());
    }

    #[test]
    fn analytic_view_rejects_duplicate_or_unused_semantic_fields() {
        let base = ChartView::Analytic {
            chart_type: AnalyticChartType::Line,
            x: "date".into(),
            y: vec!["close".into(), "close".into()],
            color: None,
            semantic_types: BTreeMap::new(),
            title: None,
            subtitle: None,
        };
        assert!(base.validate().is_err());

        let ChartView::Analytic {
            mut semantic_types, ..
        } = base
        else {
            unreachable!();
        };
        semantic_types.insert("unused".into(), "quantitative".into());
        let view = ChartView::Analytic {
            chart_type: AnalyticChartType::Line,
            x: "date".into(),
            y: vec!["close".into()],
            color: None,
            semantic_types,
            title: None,
            subtitle: None,
        };
        assert!(view.validate().is_err());

        let static_series_with_color = ChartView::Analytic {
            chart_type: AnalyticChartType::Line,
            x: "date".into(),
            y: vec!["close".into(), "volume".into()],
            color: Some("symbol".into()),
            semantic_types: BTreeMap::new(),
            title: None,
            subtitle: None,
        };
        assert!(static_series_with_color.validate().is_err());
    }

    #[test]
    fn financial_study_parameters_allow_defaults_and_reject_unsafe_shapes() {
        let mut study = ChartStudy {
            module_id: ChartStudyModule::Macd,
            calc_params: Vec::new(),
        };
        assert!(study.validate().is_ok());
        study.calc_params = vec![12.0, 26.0];
        assert!(study.validate().is_err());
        study.calc_params = vec![12.0, 26.0, 9.0];
        assert!(study.validate().is_ok());
        study.calc_params = vec![12.0, 0.0, 9.0];
        assert!(study.validate().is_err());
        study.module_id = ChartStudyModule::Ma;
        study.calc_params = vec![0.5];
        assert!(study.validate().is_err());
        study.module_id = ChartStudyModule::Boll;
        study.calc_params = vec![20.0, 2.5];
        assert!(study.validate().is_ok());
        study.module_id = ChartStudyModule::Sma;
        study.calc_params = vec![2.0, 12.0];
        assert!(study.validate().is_err());

        let supported = [
            (ChartStudyModule::Ao, vec![5.0, 34.0]),
            (ChartStudyModule::Bias, vec![6.0, 12.0, 24.0]),
            (ChartStudyModule::Brar, vec![26.0]),
            (ChartStudyModule::Bbi, vec![3.0, 6.0, 12.0, 24.0]),
            (ChartStudyModule::Cci, vec![20.0]),
            (ChartStudyModule::Cr, vec![26.0, 10.0, 20.0, 40.0, 60.0]),
            (ChartStudyModule::Dma, vec![10.0, 50.0, 10.0]),
            (ChartStudyModule::Dmi, vec![14.0, 6.0]),
            (ChartStudyModule::Emv, vec![14.0, 9.0]),
            (ChartStudyModule::Mtm, vec![12.0, 6.0]),
            (ChartStudyModule::Obv, vec![30.0]),
            (ChartStudyModule::Psy, vec![12.0, 6.0]),
            (ChartStudyModule::Roc, vec![12.0, 6.0]),
            (ChartStudyModule::Sar, vec![2.0, 2.0, 20.0]),
            (ChartStudyModule::Trix, vec![12.0, 9.0]),
            (ChartStudyModule::Vr, vec![26.0, 6.0]),
            (ChartStudyModule::Wr, vec![6.0, 10.0, 14.0]),
        ];
        for (module_id, calc_params) in supported {
            assert!(
                ChartStudy {
                    module_id,
                    calc_params,
                }
                .validate()
                .is_ok(),
                "valid parameters for {module_id:?}"
            );
        }
        for module_id in [ChartStudyModule::Avp, ChartStudyModule::Pvt] {
            assert!(ChartStudy {
                module_id,
                calc_params: vec![],
            }
            .validate()
            .is_ok());
            assert!(ChartStudy {
                module_id,
                calc_params: vec![10.0],
            }
            .validate()
            .is_err());
        }
    }

    #[test]
    fn financial_drawings_have_bounded_typed_geometry() {
        let valid = ChartDrawing {
            kind: ChartDrawingKind::Segment,
            points: vec![
                ChartDrawingPoint {
                    timestamp: json!("2026-08-01"),
                    value: 10.0,
                },
                ChartDrawingPoint {
                    timestamp: json!("2026-08-02"),
                    value: 12.0,
                },
            ],
            color: Some("#2563EB".into()),
            line_width: Some(2),
            line_style: Some(ChartDrawingLineStyle::Dashed),
            label: Some("breakout".into()),
        };
        assert!(valid.validate().is_ok());

        let mut invalid = valid.clone();
        invalid.kind = ChartDrawingKind::PriceLine;
        assert!(invalid.validate().is_err());
        invalid.points.truncate(1);
        assert!(invalid.validate().is_ok());
        invalid.color = Some("red".into());
        assert!(invalid.validate().is_err());

        let mut unlabeled_annotation = valid.clone();
        unlabeled_annotation.kind = ChartDrawingKind::Annotation;
        unlabeled_annotation.points.truncate(1);
        unlabeled_annotation.label = None;
        assert!(unlabeled_annotation.validate().is_err());
        unlabeled_annotation.label = Some("earnings".into());
        assert!(unlabeled_annotation.validate().is_ok());
        unlabeled_annotation.label = Some("earnings\ncall".into());
        assert!(unlabeled_annotation.validate().is_err());
        unlabeled_annotation.label = Some("e".repeat(81));
        assert!(unlabeled_annotation.validate().is_err());

        let mut position = valid.clone();
        position.kind = ChartDrawingKind::LongPosition;
        position.points.push(ChartDrawingPoint {
            timestamp: json!("2026-08-03"),
            value: 14.0,
        });
        position.label = None;
        assert!(position.validate().is_ok());
        position.points.pop();
        assert!(position.validate().is_err());
    }

    #[test]
    fn financial_time_contract_rejects_ambiguous_or_unrenderable_values() {
        for interval in ["1d", "15m", "1wk", "9999mo"] {
            assert!(valid_chart_interval(interval), "valid interval {interval}");
        }
        for interval in ["quarterly", "0d", "01d", "10000m", "1D"] {
            assert!(
                !valid_chart_interval(interval),
                "invalid interval {interval}"
            );
        }

        assert_eq!(
            chart_timestamp(&json!(1_786_492_800)),
            Some(1_786_492_800_000.0)
        );
        assert!(chart_timestamp(&json!(-2_208_988_800_000_i64)).is_none());
        assert!(chart_timestamp(&json!(1.0e20)).is_none());
        assert!(chart_timestamp(&json!("1900-01-01")).is_some());
    }

    #[test]
    fn financial_dataset_requires_ordered_valid_ohlc_rows() {
        let dataset = ChartDataset::new(
            "dataset-1".into(),
            ["date", "open", "high", "low", "close"]
                .into_iter()
                .map(|id| ChartColumn {
                    id: id.into(),
                    label: id.into(),
                    kind: if id == "date" {
                        ChartColumnKind::Date
                    } else {
                        ChartColumnKind::Number
                    },
                })
                .collect(),
            vec![
                vec![
                    json!("2026-08-01"),
                    json!(10),
                    json!(12),
                    json!(9),
                    json!(11),
                ],
                vec![
                    json!("2026-08-02"),
                    json!(11),
                    json!(13),
                    json!(10),
                    json!(12),
                ],
            ],
            ChartDatasetLineage::AgentAuthored {
                upstream_receipts: vec![],
            },
        )
        .unwrap();
        let document = ChartDocument {
            dataset_id: dataset.id.clone(),
            dataset_digest: dataset.digest.clone(),
            view: ChartView::Financial {
                symbol: "TEST".into(),
                interval: "1d".into(),
                time: "date".into(),
                open: "open".into(),
                high: "high".into(),
                low: "low".into(),
                close: "close".into(),
                volume: None,
                turnover: None,
                price_precision: None,
            },
            studies: Vec::new(),
            drawings: vec![ChartDrawing {
                kind: ChartDrawingKind::Segment,
                points: vec![
                    ChartDrawingPoint {
                        timestamp: json!("2026-08-01"),
                        value: 10.0,
                    },
                    ChartDrawingPoint {
                        timestamp: json!("2026-08-02"),
                        value: 12.0,
                    },
                ],
                color: None,
                line_width: None,
                line_style: None,
                label: None,
            }],
            note: None,
        };
        document.validate_dataset(&dataset).unwrap();

        let mut drawing_outside_dataset = document.clone();
        drawing_outside_dataset.drawings[0].points[1].timestamp = json!("2026-08-03");
        assert!(drawing_outside_dataset.validate_dataset(&dataset).is_err());

        let mut invalid = dataset.clone();
        invalid.rows[1][2] = json!(10);
        invalid.digest = dataset_digest(&invalid.columns, &invalid.rows, &invalid.lineage).unwrap();
        let mut invalid_document = document;
        invalid_document.dataset_digest = invalid.digest.clone();
        assert!(invalid_document.validate_dataset(&invalid).is_err());
    }
}
