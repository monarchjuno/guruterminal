use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::chart_engine::{ChartDataset, ChartDocument, CHART_SCHEMA};

const MARKDOWN_SCHEMA: &str = "guruterminal-markdown/1";
const MAX_TITLE_BYTES: usize = 200;
const MAX_MARKDOWN_BYTES: usize = 256 * 1024;
pub const MAX_CHAT_TURN_ARTIFACTS: usize = 4;

#[derive(Debug, Error)]
pub enum ChatArtifactError {
    #[error("artifact is invalid: {0}")]
    Invalid(&'static str),
    #[error("artifact digest could not be computed")]
    Digest,
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_:/.".contains(&byte))
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum && !value.contains('\0')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatArtifactKind {
    Markdown,
    Chart,
}

impl ChatArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Chart => "chart",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatArtifactRef {
    pub artifact_id: String,
    pub revision: u32,
    pub kind: ChatArtifactKind,
    pub title: String,
    pub digest: String,
}

impl ChatArtifactRef {
    pub fn validate(&self) -> Result<(), ChatArtifactError> {
        if !valid_identifier(&self.artifact_id)
            || self.revision == 0
            || !valid_text(&self.title, MAX_TITLE_BYTES)
            || self.digest.len() != 64
            || !self.digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(ChatArtifactError::Invalid("artifact reference"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatArtifact {
    pub id: String,
    pub chat_session_id: String,
    pub kind: ChatArtifactKind,
    pub title: String,
    pub current_revision: u32,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl ChatArtifact {
    pub fn validate(&self) -> Result<(), ChatArtifactError> {
        if !valid_identifier(&self.id)
            || !valid_identifier(&self.chat_session_id)
            || !valid_text(&self.title, MAX_TITLE_BYTES)
            || self.current_revision == 0
            || self.created_at_ms < 0
            || self.updated_at_ms < self.created_at_ms
        {
            return Err(ChatArtifactError::Invalid("artifact object"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatArtifactPayload {
    Markdown {
        schema: String,
        markdown: String,
    },
    Chart {
        schema: String,
        chart: Box<ChartDocument>,
    },
}

impl ChatArtifactPayload {
    pub fn kind(&self) -> ChatArtifactKind {
        match self {
            Self::Markdown { .. } => ChatArtifactKind::Markdown,
            Self::Chart { .. } => ChatArtifactKind::Chart,
        }
    }

    pub fn validate(&self) -> Result<(), ChatArtifactError> {
        match self {
            Self::Markdown { schema, markdown } => {
                if schema != MARKDOWN_SCHEMA || !valid_text(markdown, MAX_MARKDOWN_BYTES) {
                    return Err(ChatArtifactError::Invalid("Markdown payload"));
                }
            }
            Self::Chart { schema, chart } => {
                if schema != CHART_SCHEMA {
                    return Err(ChatArtifactError::Invalid("chart schema"));
                }
                chart.validate()?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatArtifactRevision {
    pub artifact_id: String,
    pub revision: u32,
    pub payload: ChatArtifactPayload,
    pub digest: String,
    pub source_message_id: String,
    pub created_at_ms: i64,
}

impl ChatArtifactRevision {
    pub fn new(
        artifact_id: String,
        revision: u32,
        payload: ChatArtifactPayload,
        source_message_id: String,
        created_at_ms: i64,
    ) -> Result<Self, ChatArtifactError> {
        payload.validate()?;
        let digest = payload_digest(&payload)?;
        let value = Self {
            artifact_id,
            revision,
            payload,
            digest,
            source_message_id,
            created_at_ms,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn artifact_ref(&self, title: String) -> ChatArtifactRef {
        ChatArtifactRef {
            artifact_id: self.artifact_id.clone(),
            revision: self.revision,
            kind: self.payload.kind(),
            title,
            digest: self.digest.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), ChatArtifactError> {
        self.payload.validate()?;
        if !valid_identifier(&self.artifact_id)
            || self.revision == 0
            || !valid_identifier(&self.source_message_id)
            || self.created_at_ms < 0
            || self.digest != payload_digest(&self.payload)?
        {
            return Err(ChatArtifactError::Invalid("artifact revision"));
        }
        Ok(())
    }
}

fn payload_digest(payload: &ChatArtifactPayload) -> Result<String, ChatArtifactError> {
    let bytes = serde_json::to_vec(payload).map_err(|_| ChatArtifactError::Digest)?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactCommit {
    pub artifact: ChatArtifact,
    pub revision: ChatArtifactRevision,
    pub datasets: Vec<ChartDataset>,
}

impl ArtifactCommit {
    pub fn validate(&self) -> Result<(), ChatArtifactError> {
        self.artifact.validate()?;
        self.revision.validate()?;
        if self.artifact.id != self.revision.artifact_id
            || self.artifact.kind != self.revision.payload.kind()
            || self.artifact.current_revision != self.revision.revision
            || self.artifact.updated_at_ms != self.revision.created_at_ms
        {
            return Err(ChatArtifactError::Invalid("artifact commit binding"));
        }
        match &self.revision.payload {
            ChatArtifactPayload::Markdown { .. } if !self.datasets.is_empty() => {
                return Err(ChatArtifactError::Invalid("Markdown artifact dataset"));
            }
            ChatArtifactPayload::Chart { chart, .. } => {
                if self.datasets.len() != 1 {
                    return Err(ChatArtifactError::Invalid("chart artifact dataset"));
                }
                let dataset = &self.datasets[0];
                chart.validate_dataset(dataset)?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::chart_engine::{AnalyticChartType, ChartColumn, ChartColumnKind, ChartView};

    fn analytic_dataset() -> ChartDataset {
        ChartDataset::new(
            "dataset-1".into(),
            vec![
                ChartColumn {
                    id: "date".into(),
                    label: "Date".into(),
                    kind: ChartColumnKind::Date,
                },
                ChartColumn {
                    id: "value".into(),
                    label: "Value".into(),
                    kind: ChartColumnKind::Number,
                },
            ],
            vec![
                vec![serde_json::json!("2026-08-01"), serde_json::json!(100.0)],
                vec![serde_json::json!("2026-08-02"), serde_json::json!(102.0)],
            ],
            crate::chart_engine::ChartDatasetLineage::AgentAuthored {
                upstream_receipts: vec![],
            },
        )
        .unwrap()
    }

    fn line_payload(dataset: &ChartDataset) -> ChatArtifactPayload {
        ChatArtifactPayload::Chart {
            schema: CHART_SCHEMA.into(),
            chart: Box::new(ChartDocument {
                dataset_id: dataset.id.clone(),
                dataset_digest: dataset.digest.clone(),
                view: ChartView::Analytic {
                    chart_type: AnalyticChartType::Line,
                    x: "date".into(),
                    y: vec!["value".into()],
                    color: None,
                    semantic_types: Default::default(),
                    title: Some("Price".into()),
                    subtitle: None,
                },
                studies: vec![],
                drawings: vec![],
                note: None,
            }),
        }
    }

    #[test]
    fn chart_revision_has_stable_digest() {
        let dataset = analytic_dataset();
        let first = ChatArtifactRevision::new(
            "artifact-1".into(),
            1,
            line_payload(&dataset),
            "message-1".into(),
            1,
        )
        .unwrap();
        let second = ChatArtifactRevision::new(
            "artifact-1".into(),
            1,
            line_payload(&dataset),
            "message-1".into(),
            1,
        )
        .unwrap();
        assert_eq!(first.digest, second.digest);
        let round_trip: ChatArtifactRevision =
            serde_json::from_str(&serde_json::to_string(&first).unwrap()).unwrap();
        assert_eq!(round_trip, first);
    }

    #[test]
    fn chart_payload_rejects_an_invalid_view() {
        let dataset = analytic_dataset();
        let mut payload = line_payload(&dataset);
        let ChatArtifactPayload::Chart { chart, .. } = &mut payload else {
            unreachable!();
        };
        let ChartView::Analytic { y, .. } = &mut chart.view else {
            unreachable!();
        };
        y.clear();
        assert!(payload.validate().is_err());
    }

    #[test]
    fn artifact_commit_binds_the_chart_to_its_separate_dataset() {
        let markdown = ChatArtifactPayload::Markdown {
            schema: MARKDOWN_SCHEMA.into(),
            markdown: "x".repeat(MAX_MARKDOWN_BYTES + 1),
        };
        assert!(markdown.validate().is_err());

        let dataset = analytic_dataset();
        let artifact = ChatArtifact {
            id: "artifact-1".into(),
            chat_session_id: "chat-1".into(),
            kind: ChatArtifactKind::Chart,
            title: "Price".into(),
            current_revision: 1,
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        let revision = ChatArtifactRevision::new(
            artifact.id.clone(),
            1,
            line_payload(&dataset),
            "message-1".into(),
            1,
        )
        .unwrap();
        let mut mismatched = dataset.clone();
        mismatched.id = "dataset-2".into();
        assert!(ArtifactCommit {
            artifact,
            revision,
            datasets: vec![mismatched],
        }
        .validate()
        .is_err());
    }
}
