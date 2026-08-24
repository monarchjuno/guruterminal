use std::collections::HashMap;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::DomainError;

const MAX_PROGRESS_TEXT_BYTES: usize = 512 * 1024;
const MAX_PROGRESS_LABEL_BYTES: usize = 512;
const MAX_PROGRESS_HREF_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatProgressStatus {
    Running,
    Succeeded,
    Failed,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatProgressCategory {
    Web,
    Memory,
    Capability,
    Finance,
    Files,
    Artifact,
    Compute,
    Decision,
    System,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatProgressOperation {
    Search,
    Read,
    Write,
    Edit,
    List,
    Calculate,
    Publish,
    Execute,
    Submit,
    Retry,
    Compact,
    Generic,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatProgressItem {
    Commentary {
        id: String,
        text: String,
    },
    Tool {
        id: String,
        category: ChatProgressCategory,
        operation: ChatProgressOperation,
        action: String,
        #[serde(deserialize_with = "crate::domain::required_option")]
        target: Option<String>,
        #[serde(deserialize_with = "crate::domain::required_option")]
        href: Option<String>,
        status: ChatProgressStatus,
        #[serde(default, rename = "startedAtMs")]
        started_at_ms: i64,
        #[serde(default, rename = "finishedAtMs")]
        #[serde(deserialize_with = "crate::domain::required_option")]
        finished_at_ms: Option<i64>,
    },
    System {
        id: String,
        category: ChatProgressCategory,
        operation: ChatProgressOperation,
        action: String,
        #[serde(deserialize_with = "crate::domain::required_option")]
        target: Option<String>,
        #[serde(deserialize_with = "crate::domain::required_option")]
        href: Option<String>,
        status: ChatProgressStatus,
        #[serde(default, rename = "startedAtMs")]
        started_at_ms: i64,
        #[serde(default, rename = "finishedAtMs")]
        #[serde(deserialize_with = "crate::domain::required_option")]
        finished_at_ms: Option<i64>,
    },
}

impl ChatProgressItem {
    fn id(&self) -> &str {
        match self {
            Self::Commentary { id, .. } | Self::Tool { id, .. } | Self::System { id, .. } => id,
        }
    }

    fn validate(&self, durable: bool) -> Result<(), DomainError> {
        validate_id(self.id())?;
        match self {
            Self::Commentary { text, .. } => validate_text(
                text,
                MAX_PROGRESS_TEXT_BYTES,
                "chat progress commentary is invalid",
            ),
            Self::Tool {
                category,
                operation,
                action,
                target,
                href,
                status,
                started_at_ms,
                finished_at_ms,
                ..
            }
            | Self::System {
                category,
                operation,
                action,
                target,
                href,
                status,
                started_at_ms,
                finished_at_ms,
                ..
            } => {
                validate_text(
                    action,
                    MAX_PROGRESS_LABEL_BYTES,
                    "chat progress action is invalid",
                )?;
                if let Some(target) = target {
                    validate_text(
                        target,
                        MAX_PROGRESS_LABEL_BYTES,
                        "chat progress target is invalid",
                    )?;
                    if target.contains('\0')
                        || target.lines().count() != 1
                        || looks_absolute_path(target)
                    {
                        return Err(DomainError::Invalid("chat progress target is unsafe"));
                    }
                }
                if let Some(href) = href {
                    validate_progress_href(href, *category, *operation, target.as_deref())?;
                }
                if matches!(self, Self::System { .. })
                    && (*category != ChatProgressCategory::System
                        || !matches!(
                            operation,
                            ChatProgressOperation::Retry
                                | ChatProgressOperation::Compact
                                | ChatProgressOperation::Generic
                        )
                        || href.is_some())
                {
                    return Err(DomainError::Invalid(
                        "chat progress system presentation is invalid",
                    ));
                }
                if *started_at_ms < 0
                    || finished_at_ms.is_some_and(|finished| finished < *started_at_ms)
                {
                    return Err(DomainError::Invalid(
                        "chat progress item timestamps are invalid",
                    ));
                }
                if durable && *status == ChatProgressStatus::Running {
                    return Err(DomainError::Invalid(
                        "durable chat progress contains a running item",
                    ));
                }
                if durable && finished_at_ms.is_none() {
                    return Err(DomainError::Invalid(
                        "durable chat progress is missing an item finish time",
                    ));
                }
                Ok(())
            }
        }
    }
}

fn validate_progress_href(
    href: &str,
    category: ChatProgressCategory,
    operation: ChatProgressOperation,
    target: Option<&str>,
) -> Result<(), DomainError> {
    if href.len() > MAX_PROGRESS_HREF_BYTES
        || category != ChatProgressCategory::Web
        || operation != ChatProgressOperation::Read
        || target.is_none()
    {
        return Err(DomainError::Invalid("chat progress link is invalid"));
    }
    let url = reqwest::Url::parse(href)
        .map_err(|_| DomainError::Invalid("chat progress link is invalid"))?;
    let port_is_allowed = matches!(
        (url.scheme(), url.port()),
        ("http", None | Some(80)) | ("https", None | Some(443))
    );
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !port_is_allowed
    {
        return Err(DomainError::Invalid("chat progress link is unsafe"));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatProgress {
    pub started_at_ms: i64,
    #[serde(deserialize_with = "crate::domain::required_option")]
    pub finished_at_ms: Option<i64>,
    pub items: Vec<ChatProgressItem>,
}

impl ChatProgress {
    pub fn validate(&self, durable: bool) -> Result<(), DomainError> {
        if self.started_at_ms < 0
            || self
                .finished_at_ms
                .is_some_and(|finished| finished < self.started_at_ms)
            || (durable && self.finished_at_ms.is_none())
        {
            return Err(DomainError::Invalid("chat progress timestamps are invalid"));
        }
        let mut ids = std::collections::BTreeSet::new();
        for item in &self.items {
            item.validate(durable)?;
            if !ids.insert(item.id()) {
                return Err(DomainError::Invalid("chat progress item id is duplicated"));
            }
        }
        Ok(())
    }
}

fn validate_id(value: &str) -> Result<(), DomainError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(DomainError::Invalid("chat progress item id is unsafe"));
    }
    Ok(())
}

fn validate_text(value: &str, max: usize, message: &'static str) -> Result<(), DomainError> {
    if value.trim().is_empty() || value.len() > max || value.contains('\0') {
        return Err(DomainError::Invalid(message));
    }
    Ok(())
}

#[derive(Debug)]
pub struct ChatProgressProjection {
    progress: ChatProgress,
    next_item: usize,
    active_commentary: Option<usize>,
    tool_items: HashMap<String, usize>,
    system_items: HashMap<String, usize>,
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ToolPresentation {
    category: ChatProgressCategory,
    operation: ChatProgressOperation,
    action: String,
    target: Option<String>,
    href: Option<String>,
}

impl ChatProgressProjection {
    pub fn new(started_at_ms: i64) -> Self {
        Self {
            progress: ChatProgress {
                started_at_ms,
                finished_at_ms: None,
                items: Vec::new(),
            },
            next_item: 1,
            active_commentary: None,
            tool_items: HashMap::new(),
            system_items: HashMap::new(),
        }
    }

    fn close_commentary(&mut self) {
        self.active_commentary = None;
    }

    pub fn start_assistant_turn(&mut self) {
        self.close_commentary();
    }

    pub fn append_commentary(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let index = match self.active_commentary {
            Some(index) => index,
            None => {
                let index = self.progress.items.len();
                let id = self.next_id("commentary");
                self.progress.items.push(ChatProgressItem::Commentary {
                    id,
                    text: String::new(),
                });
                self.active_commentary = Some(index);
                index
            }
        };
        if let ChatProgressItem::Commentary { text, .. } = &mut self.progress.items[index] {
            text.push_str(delta);
        }
    }

    pub fn finish_assistant_turn(&mut self, is_final: bool) {
        if is_final {
            if let Some(index) = self.active_commentary.take() {
                self.progress.items.remove(index);
            }
        } else {
            self.close_commentary();
        }
    }

    pub fn start_tool(
        &mut self,
        tool_call_id: &str,
        tool_name: &str,
        args: &Value,
        web_source: Option<&crate::web::WebSource>,
        at_ms: i64,
    ) {
        self.close_commentary();
        let presentation = summarize_tool(tool_name, args, web_source);
        let id = self.next_id("tool");
        let index = self.progress.items.len();
        self.progress.items.push(ChatProgressItem::Tool {
            id,
            category: presentation.category,
            operation: presentation.operation,
            action: presentation.action,
            target: presentation.target,
            href: presentation.href,
            status: ChatProgressStatus::Running,
            started_at_ms: at_ms.max(0),
            finished_at_ms: None,
        });
        self.tool_items.insert(tool_call_id.to_owned(), index);
    }

    pub fn finish_tool(&mut self, tool_call_id: &str, failed: bool, at_ms: i64) {
        let Some(index) = self.tool_items.remove(tool_call_id) else {
            return;
        };
        if let Some(ChatProgressItem::Tool {
            status,
            started_at_ms,
            finished_at_ms,
            ..
        }) = self.progress.items.get_mut(index)
        {
            *status = if failed {
                ChatProgressStatus::Failed
            } else {
                ChatProgressStatus::Succeeded
            };
            *finished_at_ms = Some(at_ms.max(*started_at_ms));
        }
    }

    pub fn start_system(
        &mut self,
        key: &str,
        operation: ChatProgressOperation,
        action: &str,
        target: Option<String>,
        at_ms: i64,
    ) {
        self.close_commentary();
        let id = self.next_id("system");
        let index = self.progress.items.len();
        self.progress.items.push(ChatProgressItem::System {
            id,
            category: ChatProgressCategory::System,
            operation,
            action: action.to_owned(),
            target,
            href: None,
            status: ChatProgressStatus::Running,
            started_at_ms: at_ms.max(0),
            finished_at_ms: None,
        });
        self.system_items.insert(key.to_owned(), index);
    }

    pub fn finish_system(&mut self, key: &str, failed: bool, at_ms: i64) {
        let Some(index) = self.system_items.remove(key) else {
            return;
        };
        if let Some(ChatProgressItem::System {
            status,
            started_at_ms,
            finished_at_ms,
            ..
        }) = self.progress.items.get_mut(index)
        {
            *status = if failed {
                ChatProgressStatus::Failed
            } else {
                ChatProgressStatus::Succeeded
            };
            *finished_at_ms = Some(at_ms.max(*started_at_ms));
        }
    }

    pub fn snapshot(&self) -> ChatProgress {
        self.progress.clone()
    }

    pub fn finish(&mut self, finished_at_ms: i64, stopped: bool) -> Option<ChatProgress> {
        self.close_commentary();
        let closed_at = finished_at_ms.max(self.progress.started_at_ms);
        self.progress.finished_at_ms = Some(closed_at);
        for item in &mut self.progress.items {
            match item {
                ChatProgressItem::Tool {
                    status,
                    started_at_ms,
                    finished_at_ms,
                    ..
                }
                | ChatProgressItem::System {
                    status,
                    started_at_ms,
                    finished_at_ms,
                    ..
                } => {
                    if *status == ChatProgressStatus::Running {
                        *status = if stopped {
                            ChatProgressStatus::Stopped
                        } else {
                            ChatProgressStatus::Failed
                        };
                    }
                    if finished_at_ms.is_none() {
                        *finished_at_ms = Some(closed_at.max(*started_at_ms));
                    }
                }
                _ => {}
            }
        }
        (!self.progress.items.is_empty()).then(|| self.progress.clone())
    }

    fn next_id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}-{}", self.next_item);
        self.next_item += 1;
        id
    }
}

fn summarize_tool(
    tool_name: &str,
    args: &Value,
    web_source: Option<&crate::web::WebSource>,
) -> ToolPresentation {
    let string = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| args.get(*key).and_then(Value::as_str))
            .and_then(safe_value)
    };
    let path = || {
        ["path", "file", "directory"]
            .iter()
            .find_map(|key| args.get(*key).and_then(Value::as_str))
            .and_then(safe_relative_path)
    };
    let presentation = |category, operation, action: &str, target| ToolPresentation {
        category,
        operation,
        action: action.into(),
        target,
        href: None,
    };
    match tool_name {
        "memory_search" => presentation(
            ChatProgressCategory::Memory,
            ChatProgressOperation::Search,
            "Searched Memory",
            string(&["query"]),
        ),
        "memory_read" => presentation(
            ChatProgressCategory::Memory,
            ChatProgressOperation::Read,
            "Read Memory",
            string(&["section"]),
        ),
        "memory_previous" => presentation(
            ChatProgressCategory::Memory,
            ChatProgressOperation::Read,
            "Read previous Memory",
            string(&["id"]),
        ),
        "capability_search" => presentation(
            ChatProgressCategory::Capability,
            ChatProgressOperation::Search,
            "Searched tools",
            string(&["query"]),
        ),
        "capability_load" => presentation(
            ChatProgressCategory::Capability,
            ChatProgressOperation::Read,
            "Opened a tool",
            string(&["id"]),
        ),
        "web_search" => presentation(
            ChatProgressCategory::Web,
            ChatProgressOperation::Search,
            "Searched the web",
            string(&["query"]),
        ),
        "web_fetch" => {
            let (target, href) = web_source
                .and_then(safe_web_source)
                .map_or((None, None), |(target, href)| (Some(target), Some(href)));
            ToolPresentation {
                category: ChatProgressCategory::Web,
                operation: ChatProgressOperation::Read,
                action: "Read a web source".into(),
                target,
                href,
            }
        }
        "finance_sources" => presentation(
            ChatProgressCategory::Finance,
            ChatProgressOperation::List,
            "Checked finance sources",
            string(&["provider"]),
        ),
        "finance_macro_data" => presentation(
            ChatProgressCategory::Finance,
            ChatProgressOperation::Read,
            "Read macro data",
            join_values(args, &["provider", "series", "series_id", "start", "end"]),
        ),
        "finance_market_data" | "finance_company_data" | "finance_filings" => presentation(
            ChatProgressCategory::Finance,
            ChatProgressOperation::Read,
            "Read financial data",
            join_values(
                &flatten_params(args),
                &[
                    "provider",
                    "operation_id",
                    "symbol",
                    "ticker",
                    "series",
                    "cik",
                    "corp_code",
                    "start",
                    "end",
                ],
            ),
        ),
        "finance_calculate" => presentation(
            ChatProgressCategory::Finance,
            ChatProgressOperation::Calculate,
            "Calculated financial data",
            string(&["operation", "calculation", "kind"]),
        ),
        "finance_resolve_entity" => presentation(
            ChatProgressCategory::Finance,
            ChatProgressOperation::Search,
            "Resolved a finance entity",
            string(&["query"]),
        ),
        "run_results_list" => presentation(
            ChatProgressCategory::Finance,
            ChatProgressOperation::List,
            "Listed current-run results",
            None,
        ),
        "artifact_list" => presentation(
            ChatProgressCategory::Artifact,
            ChatProgressOperation::List,
            "Listed Chat artifacts",
            None,
        ),
        "artifact_read" => presentation(
            ChatProgressCategory::Artifact,
            ChatProgressOperation::Read,
            "Read a Chat artifact",
            join_values(args, &["artifact_id", "id", "revision"]),
        ),
        "artifact_publish" => presentation(
            ChatProgressCategory::Artifact,
            ChatProgressOperation::Publish,
            "Published a Chat artifact",
            join_values(args, &["title", "artifact_id", "id", "mode", "action"]),
        ),
        "chart_query" => presentation(
            ChatProgressCategory::Artifact,
            ChatProgressOperation::Read,
            "Read chart data",
            join_values(args, &["artifact_id", "revision"]),
        ),
        "chart_publish" => presentation(
            ChatProgressCategory::Artifact,
            ChatProgressOperation::Publish,
            "Published a chart",
            join_values(args, &["title", "artifact_id", "mode"]),
        ),
        "memory_patch_propose" => presentation(
            ChatProgressCategory::Memory,
            ChatProgressOperation::Edit,
            "Drafted a Memory update",
            string(&["section"]),
        ),
        "read" => presentation(
            ChatProgressCategory::Files,
            ChatProgressOperation::Read,
            "Read a file",
            path(),
        ),
        "write" => presentation(
            ChatProgressCategory::Files,
            ChatProgressOperation::Write,
            "Wrote a file",
            path(),
        ),
        "edit" => presentation(
            ChatProgressCategory::Files,
            ChatProgressOperation::Edit,
            "Edited a file",
            path(),
        ),
        "ls" => presentation(
            ChatProgressCategory::Files,
            ChatProgressOperation::List,
            "Listed files",
            path(),
        ),
        "find" => presentation(
            ChatProgressCategory::Files,
            ChatProgressOperation::Search,
            "Searched files",
            path().or_else(|| string(&["pattern", "query"])),
        ),
        "grep" => presentation(
            ChatProgressCategory::Files,
            ChatProgressOperation::Search,
            "Searched file contents",
            path().or_else(|| string(&["pattern", "query"])),
        ),
        "compute_run" => presentation(
            ChatProgressCategory::Compute,
            ChatProgressOperation::Execute,
            "Ran a sandboxed calculation",
            package_names(args).or_else(|| string(&["language"])),
        ),
        "decision_submit" => presentation(
            ChatProgressCategory::Decision,
            ChatProgressOperation::Submit,
            "Submitted a decision",
            None,
        ),
        "evidence_create" => presentation(
            ChatProgressCategory::Memory,
            ChatProgressOperation::Publish,
            "Created evidence",
            string(&["title"]),
        ),
        name if name.starts_with("mcp__") => presentation(
            ChatProgressCategory::Finance,
            ChatProgressOperation::Read,
            "Read market data",
            join_values(
                &flatten_params(args),
                &[
                    "symbol", "ticker", "provider", "query", "cik", "start", "end",
                ],
            ),
        ),
        _ => presentation(
            ChatProgressCategory::Capability,
            ChatProgressOperation::Generic,
            &safe_tool_name(tool_name),
            None,
        ),
    }
}

fn safe_web_source(source: &crate::web::WebSource) -> Option<(String, String)> {
    let url = reqwest::Url::parse(&source.url).ok()?;
    let host = url.host_str()?;
    validate_progress_href(
        &source.url,
        ChatProgressCategory::Web,
        ChatProgressOperation::Read,
        Some("source"),
    )
    .ok()?;
    let title = safe_web_title(&source.title).unwrap_or_else(|| host.to_owned());
    Some((format!("{title} · {host}"), source.url.clone()))
}

fn safe_web_title(value: &str) -> Option<String> {
    let cleaned = value
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    safe_value(&cleaned)
}

fn safe_tool_name(value: &str) -> String {
    let name = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        .take(64)
        .collect::<String>()
        .replace(['_', '-'], " ");
    if name.trim().is_empty() {
        "Used a tool".into()
    } else {
        format!("Ran {name}")
    }
}

fn safe_value(value: &str) -> Option<String> {
    let value = value.trim();
    let lowercase = value.to_ascii_lowercase();
    if value.is_empty()
        || value.contains(['\0', '\n', '\r'])
        || looks_absolute_path(value)
        || [
            "api_key",
            "apikey",
            "authorization",
            "bearer ",
            "credential",
            "password",
            "secret",
            "token=",
        ]
        .iter()
        .any(|marker| lowercase.contains(marker))
    {
        return None;
    }
    Some(truncate_utf8(value, 160).to_owned())
}

fn safe_relative_path(value: &str) -> Option<String> {
    let value = value.trim();
    let path = Path::new(value);
    if path.as_os_str().is_empty()
        || looks_absolute_path(value)
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }
    safe_value(value)
}

fn looks_absolute_path(value: &str) -> bool {
    let value = value.trim();
    Path::new(value).is_absolute()
        || value.starts_with(['~', '\\'])
        || value.as_bytes().get(1) == Some(&b':')
}

fn flatten_params(args: &Value) -> Value {
    let mut combined = args.clone();
    let Some(params) = args.get("params").and_then(Value::as_object) else {
        return combined;
    };
    if let Some(object) = combined.as_object_mut() {
        for (key, value) in params {
            object.entry(key.clone()).or_insert(value.clone());
        }
    }
    combined
}

fn join_values(args: &Value, keys: &[&str]) -> Option<String> {
    let values = keys
        .iter()
        .filter_map(|key| args.get(*key))
        .filter_map(|value| match value {
            Value::String(value) => safe_value(value),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| truncate_utf8(&values.join(" · "), 480).to_owned())
}

fn package_names(args: &Value) -> Option<String> {
    let packages = args
        .get("packages")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_str)
        .filter_map(safe_value)
        .take(12)
        .collect::<Vec<_>>();
    (!packages.is_empty()).then(|| truncate_utf8(&packages.join(", "), 480).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_parallel_tool_order_and_updates_by_call_id() {
        let mut projection = ChatProgressProjection::new(10);
        projection.start_tool("call-b", "web_search", &json!({"query":"rates"}), None, 10);
        projection.start_tool(
            "call-a",
            "memory_read",
            &json!({"record_id":"lens:rates"}),
            None,
            11,
        );
        projection.finish_tool("call-a", false, 15);
        projection.finish_tool("call-b", true, 16);
        let snapshot = projection.finish(20, false).unwrap();
        assert!(matches!(
            snapshot.items[0],
            ChatProgressItem::Tool {
                status: ChatProgressStatus::Failed,
                ..
            }
        ));
        assert!(matches!(
            snapshot.items[1],
            ChatProgressItem::Tool {
                status: ChatProgressStatus::Succeeded,
                ..
            }
        ));
    }

    #[test]
    fn final_turn_is_removed_but_tool_use_commentary_remains() {
        let mut projection = ChatProgressProjection::new(10);
        projection.start_assistant_turn();
        projection.append_commentary("I will inspect the files.");
        projection.finish_assistant_turn(false);
        projection.start_assistant_turn();
        projection.append_commentary("The change is complete.");
        projection.finish_assistant_turn(true);
        let snapshot = projection.finish(20, false).unwrap();
        assert_eq!(snapshot.items.len(), 1);
        assert!(
            matches!(&snapshot.items[0], ChatProgressItem::Commentary { text, .. } if text == "I will inspect the files.")
        );
    }

    #[test]
    fn commentary_after_a_tool_starts_a_new_timeline_item() {
        let mut projection = ChatProgressProjection::new(10);
        projection.start_assistant_turn();
        projection.append_commentary("I will search first.");
        projection.start_tool("call-1", "web_search", &json!({"query": "btc"}), None, 11);
        projection.finish_tool("call-1", false, 12);
        projection.append_commentary("The source is enough to continue.");
        projection.start_tool(
            "call-2",
            "memory_read",
            &json!({"record_id": "lens:rates"}),
            None,
            13,
        );
        projection.finish_assistant_turn(false);
        let snapshot = projection.snapshot();
        assert_eq!(snapshot.items.len(), 4);
        assert!(
            matches!(&snapshot.items[0], ChatProgressItem::Commentary { text, .. } if text == "I will search first.")
        );
        assert!(matches!(&snapshot.items[1], ChatProgressItem::Tool { .. }));
        assert!(
            matches!(&snapshot.items[2], ChatProgressItem::Commentary { text, .. } if text == "The source is enough to continue.")
        );
        assert!(matches!(&snapshot.items[3], ChatProgressItem::Tool { .. }));
    }

    #[test]
    fn summaries_drop_absolute_paths_compute_source_and_unknown_arguments() {
        let path = summarize_tool("read", &json!({"path":"/Users/me/secret.txt"}), None);
        assert!(path.target.is_none());
        let compute = summarize_tool(
            "compute_run",
            &json!({"packages":["numpy"],"source":"print(secret)","inputs":{"token":"secret"}}),
            None,
        );
        assert_eq!(compute.target.as_deref(), Some("numpy"));
        let memory = summarize_tool("memory_read", &json!({"record_id":"lens:rates"}), None);
        assert!(memory.target.is_none());
        let capability_load = summarize_tool(
            "capability_load",
            &json!({"capability_id":"guruterminal.finance-core"}),
            None,
        );
        assert!(capability_load.target.is_none());
        let capability_load_id = summarize_tool(
            "capability_load",
            &json!({"id":"community.web-research/research"}),
            None,
        );
        assert_eq!(
            capability_load_id.target.as_deref(),
            Some("community.web-research/research")
        );
        let capability = summarize_tool("capability_secret", &json!({"token":"credential"}), None);
        assert!(capability.target.is_none());
        let openbb = summarize_tool(
            "mcp__openbb__equity_price_historical",
            &json!({"symbol":"AAPL","provider":"yfinance"}),
            None,
        );
        assert_eq!(openbb.action, "Read market data");
        assert_eq!(openbb.target.as_deref(), Some("AAPL · yfinance"));
        let query = summarize_tool(
            "web_search",
            &json!({"query":"authorization: Bearer credential"}),
            None,
        );
        assert!(query.target.is_none());
        let windows_path =
            summarize_tool("read", &json!({"path":"C:\\Users\\me\\secret.txt"}), None);
        assert!(windows_path.target.is_none());
        let kis = summarize_tool(
            "finance_market_data",
            &json!({
                "provider": "koreainvestment.market-data",
                "operation_id": "domestic_stock.inquire_price",
                "params": {"fid_input_iscd": "005930"}
            }),
            None,
        );
        assert_eq!(
            kis.target.as_deref(),
            Some("koreainvestment.market-data · domestic_stock.inquire_price")
        );
    }

    #[test]
    fn classifies_known_tools_into_typed_categories_and_operations() {
        let cases = [
            (
                "memory_search",
                ChatProgressCategory::Memory,
                ChatProgressOperation::Search,
            ),
            (
                "capability_load",
                ChatProgressCategory::Capability,
                ChatProgressOperation::Read,
            ),
            (
                "web_search",
                ChatProgressCategory::Web,
                ChatProgressOperation::Search,
            ),
            (
                "finance_calculate",
                ChatProgressCategory::Finance,
                ChatProgressOperation::Calculate,
            ),
            (
                "artifact_publish",
                ChatProgressCategory::Artifact,
                ChatProgressOperation::Publish,
            ),
            (
                "edit",
                ChatProgressCategory::Files,
                ChatProgressOperation::Edit,
            ),
            (
                "compute_run",
                ChatProgressCategory::Compute,
                ChatProgressOperation::Execute,
            ),
            (
                "decision_submit",
                ChatProgressCategory::Decision,
                ChatProgressOperation::Submit,
            ),
            (
                "other_bundled_tool",
                ChatProgressCategory::Capability,
                ChatProgressOperation::Generic,
            ),
            (
                "chart_query",
                ChatProgressCategory::Artifact,
                ChatProgressOperation::Read,
            ),
            (
                "chart_publish",
                ChatProgressCategory::Artifact,
                ChatProgressOperation::Publish,
            ),
            (
                "mcp__openbb__equity_price_historical",
                ChatProgressCategory::Finance,
                ChatProgressOperation::Read,
            ),
        ];
        for (tool_name, category, operation) in cases {
            let presentation = summarize_tool(tool_name, &json!({}), None);
            assert_eq!(presentation.category, category, "{tool_name}");
            assert_eq!(presentation.operation, operation, "{tool_name}");
        }

        let mut projection = ChatProgressProjection::new(1);
        projection.start_system(
            "retry",
            ChatProgressOperation::Retry,
            "Retrying model request",
            None,
            1,
        );
        assert!(matches!(
            projection.snapshot().items.as_slice(),
            [ChatProgressItem::System {
                category: ChatProgressCategory::System,
                operation: ChatProgressOperation::Retry,
                ..
            }]
        ));
    }

    #[test]
    fn stop_finalizes_running_items_without_raw_results() {
        let mut projection = ChatProgressProjection::new(10);
        projection.start_tool(
            "call",
            "web_fetch",
            &json!({"source_id":"source-1"}),
            None,
            10,
        );
        let snapshot = projection.finish(20, true).unwrap();
        assert!(matches!(
            snapshot.items[0],
            ChatProgressItem::Tool {
                status: ChatProgressStatus::Stopped,
                ..
            }
        ));
        assert!(!serde_json::to_string(&snapshot)
            .unwrap()
            .contains("source-1"));
    }

    #[test]
    fn web_source_uses_a_safe_title_domain_and_link_without_the_internal_id() {
        let source = crate::web::WebSource {
            source_id: "web:f6495eccf2c0cca04e4c6a03".into(),
            title: "Quarterly market report".into(),
            url: "https://example.com/reports/q1".into(),
            snippet: "raw provider text".into(),
            published_at: None,
        };
        let mut projection = ChatProgressProjection::new(10);
        projection.start_tool(
            "call",
            "web_fetch",
            &json!({"source_id": source.source_id}),
            Some(&source),
            10,
        );
        projection.finish_tool("call", false, 18);
        let snapshot = projection.finish(20, false).unwrap();
        let serialized = serde_json::to_string(&snapshot).unwrap();
        assert!(serialized.contains("Quarterly market report · example.com"));
        assert!(serialized.contains("https://example.com/reports/q1"));
        assert!(!serialized.contains("web:f6495ecc"));
        assert!(!serialized.contains("raw provider text"));
    }

    #[test]
    fn web_source_strips_control_characters_and_rejects_unsafe_links() {
        let safe = crate::web::WebSource {
            source_id: "web:safe".into(),
            title: "Quarterly\u{0001} report".into(),
            url: "https://example.com/report".into(),
            snippet: "hidden".into(),
            published_at: None,
        };
        let presentation = summarize_tool("web_fetch", &json!({}), Some(&safe));
        assert_eq!(
            presentation.target.as_deref(),
            Some("Quarterly report · example.com")
        );

        let unsafe_source = crate::web::WebSource {
            source_id: "web:unsafe".into(),
            title: "Private source".into(),
            url: "https://user:password@example.com/report".into(),
            snippet: "hidden".into(),
            published_at: None,
        };
        let presentation = summarize_tool("web_fetch", &json!({}), Some(&unsafe_source));
        assert!(presentation.target.is_none());
        assert!(presentation.href.is_none());
    }
}
