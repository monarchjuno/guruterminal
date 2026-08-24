use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use tauri::{
    ipc::Channel,
    webview::{DownloadEvent, NewWindowResponse, PageLoadEvent, WebviewBuilder},
    LogicalPosition, LogicalSize, Manager, State, Webview, WebviewUrl, Window,
};
use uuid::Uuid;

use crate::app::{AppState, CommandError};

const MAX_BROWSER_TABS: usize = 12;
const MAX_URL_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl BrowserBounds {
    fn constrained(&self, max_width: f64, max_height: f64) -> Result<Self, CommandError> {
        if ![
            self.x,
            self.y,
            self.width,
            self.height,
            max_width,
            max_height,
        ]
        .into_iter()
        .all(f64::is_finite)
            || self.x < 0.0
            || self.y < 0.0
            || self.width < 1.0
            || self.height < 1.0
            || max_width < 1.0
            || max_height < 1.0
        {
            return Err(CommandError::invalid("browser bounds are invalid"));
        }
        let x = self.x.min(max_width - 1.0);
        let y = self.y.min(max_height - 1.0);
        Ok(Self {
            x,
            y,
            width: self.width.min(max_width - x).max(1.0),
            height: self.height.min(max_height - y).max(1.0),
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BrowserTabState {
    pub tab_id: String,
    pub url: String,
    pub title: String,
    pub loading: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserTabEvent {
    LoadStarted {
        tab_id: String,
        url: String,
    },
    LoadFinished {
        tab_id: String,
        url: String,
    },
    TitleChanged {
        tab_id: String,
        title: String,
    },
    OpenRequested {
        tab_id: String,
        url: String,
    },
    NavigationBlocked {
        tab_id: String,
        url: String,
        message: String,
    },
    DownloadBlocked {
        tab_id: String,
        url: String,
    },
}

fn queue_browser_event(channel: Channel<BrowserTabEvent>, event: BrowserTabEvent) {
    // Wry invokes navigation and page callbacks while its window registry is borrowed.
    // Channel::send evaluates JavaScript in the main webview, which would re-enter that
    // registry and panic on macOS. Tokio never polls a spawned task synchronously, so
    // sending from the runtime worker queues the WebView operation after the callback.
    tauri::async_runtime::spawn(async move {
        let _ = channel.send(event);
    });
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserTabOpenRequest {
    pub url: String,
    pub bounds: BrowserBounds,
    pub visible: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserTabRequest {
    pub tab_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserTabNavigateRequest {
    pub tab_id: String,
    pub url: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserHistoryDirection {
    Back,
    Forward,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserTabHistoryRequest {
    pub tab_id: String,
    pub direction: BrowserHistoryDirection,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserTabBoundsRequest {
    pub tab_id: String,
    pub bounds: BrowserBounds,
    pub visible: bool,
}

#[derive(Clone, Debug)]
struct BrowserRecord {
    label: String,
    url: String,
    title: String,
    loading: bool,
}

impl BrowserRecord {
    fn state(&self, tab_id: &str) -> BrowserTabState {
        BrowserTabState {
            tab_id: tab_id.to_owned(),
            url: self.url.clone(),
            title: self.title.clone(),
            loading: self.loading,
        }
    }
}

#[derive(Clone)]
pub struct BrowserManager {
    tabs: Arc<Mutex<HashMap<String, BrowserRecord>>>,
    profile_dir: PathBuf,
}

impl BrowserManager {
    pub fn new(profile_dir: PathBuf) -> Self {
        Self {
            tabs: Arc::new(Mutex::new(HashMap::new())),
            profile_dir,
        }
    }

    fn reserve(&self, url: &tauri::Url) -> Result<BrowserTabState, CommandError> {
        let mut tabs = self
            .tabs
            .lock()
            .map_err(|_| CommandError::internal("browser registry lock was poisoned"))?;
        if tabs.len() >= MAX_BROWSER_TABS {
            return Err(CommandError::conflict(
                "close a browser tab before opening another one",
            ));
        }
        let tab_id = Uuid::new_v4().to_string();
        let record = BrowserRecord {
            label: format!("browser-{tab_id}"),
            url: url.as_str().to_owned(),
            title: url.host_str().unwrap_or("Web page").to_owned(),
            loading: true,
        };
        let state = record.state(&tab_id);
        tabs.insert(tab_id, record);
        Ok(state)
    }

    fn label(&self, tab_id: &str) -> Result<String, CommandError> {
        validate_tab_id(tab_id)?;
        self.tabs
            .lock()
            .map_err(|_| CommandError::internal("browser registry lock was poisoned"))?
            .get(tab_id)
            .map(|record| record.label.clone())
            .ok_or_else(|| CommandError::not_found("Browser tab"))
    }

    fn remove(&self, tab_id: &str) -> Result<Option<BrowserRecord>, CommandError> {
        validate_tab_id(tab_id)?;
        Ok(self
            .tabs
            .lock()
            .map_err(|_| CommandError::internal("browser registry lock was poisoned"))?
            .remove(tab_id))
    }

    fn reset(&self) -> Result<Vec<BrowserRecord>, CommandError> {
        let mut tabs = self
            .tabs
            .lock()
            .map_err(|_| CommandError::internal("browser registry lock was poisoned"))?;
        Ok(tabs.drain().map(|(_, record)| record).collect())
    }

    fn can_request_popup(&self) -> bool {
        self.tabs
            .lock()
            .map(|tabs| tabs.len() < MAX_BROWSER_TABS)
            .unwrap_or(false)
    }

    fn update_load(&self, tab_id: &str, url: &str, loading: bool) {
        if let Ok(mut tabs) = self.tabs.lock() {
            if let Some(record) = tabs.get_mut(tab_id) {
                record.url = url.to_owned();
                record.loading = loading;
            }
        }
    }

    fn update_title(&self, tab_id: &str, title: &str) {
        if let Ok(mut tabs) = self.tabs.lock() {
            if let Some(record) = tabs.get_mut(tab_id) {
                record.title = title.chars().take(240).collect();
            }
        }
    }
}

pub fn validated_http_url(raw: &str) -> Result<tauri::Url, CommandError> {
    if raw.len() > MAX_URL_BYTES {
        return Err(CommandError::invalid("external link URL is too long"));
    }
    let url = tauri::Url::parse(raw)
        .map_err(|_| CommandError::invalid("external link URL is invalid"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(CommandError::invalid(
            "only http and https links without a password can be opened",
        ));
    }
    Ok(url)
}

fn validate_tab_id(tab_id: &str) -> Result<(), CommandError> {
    Uuid::parse_str(tab_id)
        .map(|_| ())
        .map_err(|_| CommandError::invalid("browser tab id is invalid"))
}

fn main_window(webview: &Webview) -> Result<Window, CommandError> {
    if webview.label() != "main" {
        return Err(CommandError::invalid(
            "browser commands are available only to the main webview",
        ));
    }
    let window = webview.window();
    if window.label() != "main" {
        return Err(CommandError::invalid(
            "browser commands are available only in the main window",
        ));
    }
    Ok(window)
}

fn constrained_bounds(
    window: &Window,
    bounds: &BrowserBounds,
) -> Result<BrowserBounds, CommandError> {
    let scale = window
        .scale_factor()
        .map_err(|error| CommandError::internal(error.to_string()))?;
    let size = window
        .inner_size()
        .map_err(|error| CommandError::internal(error.to_string()))?
        .to_logical::<f64>(scale);
    bounds.constrained(size.width, size.height)
}

fn browser_webview(
    window: &Window,
    manager: &BrowserManager,
    tab_id: &str,
) -> Result<tauri::Webview, CommandError> {
    let label = manager.label(tab_id)?;
    window
        .get_webview(&label)
        .ok_or_else(|| CommandError::not_found("Browser webview"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn browser_tab_open(
    request: BrowserTabOpenRequest,
    on_event: Channel<BrowserTabEvent>,
    webview: Webview,
    state: State<'_, AppState>,
) -> Result<BrowserTabState, CommandError> {
    let window = main_window(&webview)?;
    let url = validated_http_url(&request.url)?;
    let bounds = constrained_bounds(&window, &request.bounds)?;
    let initial = state.browser.reserve(&url)?;
    let tab_id = initial.tab_id.clone();
    let label = state.browser.label(&tab_id)?;

    let navigation_tab_id = tab_id.clone();
    let navigation_events = on_event.clone();
    let load_tab_id = tab_id.clone();
    let load_events = on_event.clone();
    let load_manager = state.browser.clone();
    let title_tab_id = tab_id.clone();
    let title_events = on_event.clone();
    let title_manager = state.browser.clone();
    let popup_tab_id = tab_id.clone();
    let popup_events = on_event.clone();
    let popup_manager = state.browser.clone();
    let download_tab_id = tab_id.clone();
    let download_events = on_event;

    let builder = WebviewBuilder::new(&label, WebviewUrl::External(url))
        .data_directory(state.browser.profile_dir.clone())
        .incognito(false)
        .on_navigation(move |next| match validated_http_url(next.as_str()) {
            Ok(_) => true,
            Err(error) => {
                queue_browser_event(
                    navigation_events.clone(),
                    BrowserTabEvent::NavigationBlocked {
                        tab_id: navigation_tab_id.clone(),
                        url: next.as_str().to_owned(),
                        message: error.message,
                    },
                );
                false
            }
        })
        .on_page_load(move |_webview, payload| {
            let url = payload.url().as_str().to_owned();
            let loading = matches!(payload.event(), PageLoadEvent::Started);
            load_manager.update_load(&load_tab_id, &url, loading);
            let event = if loading {
                BrowserTabEvent::LoadStarted {
                    tab_id: load_tab_id.clone(),
                    url,
                }
            } else {
                BrowserTabEvent::LoadFinished {
                    tab_id: load_tab_id.clone(),
                    url,
                }
            };
            queue_browser_event(load_events.clone(), event);
        })
        .on_document_title_changed(move |_webview, title| {
            title_manager.update_title(&title_tab_id, &title);
            queue_browser_event(
                title_events.clone(),
                BrowserTabEvent::TitleChanged {
                    tab_id: title_tab_id.clone(),
                    title: title.chars().take(240).collect(),
                },
            );
        })
        .on_new_window(move |next, _features| {
            if popup_manager.can_request_popup() && validated_http_url(next.as_str()).is_ok() {
                queue_browser_event(
                    popup_events.clone(),
                    BrowserTabEvent::OpenRequested {
                        tab_id: popup_tab_id.clone(),
                        url: next.as_str().to_owned(),
                    },
                );
            }
            NewWindowResponse::Deny
        })
        .on_download(move |_webview, event| {
            if let DownloadEvent::Requested { url, .. } = event {
                queue_browser_event(
                    download_events.clone(),
                    BrowserTabEvent::DownloadBlocked {
                        tab_id: download_tab_id.clone(),
                        url: url.as_str().to_owned(),
                    },
                );
            }
            false
        });

    let webview = match window.add_child(
        builder,
        LogicalPosition::new(bounds.x, bounds.y),
        LogicalSize::new(bounds.width, bounds.height),
    ) {
        Ok(webview) => webview,
        Err(error) => {
            let _ = state.browser.remove(&tab_id);
            return Err(CommandError::internal(format!(
                "could not open browser tab: {error}"
            )));
        }
    };
    if !request.visible {
        if let Err(error) = webview.hide() {
            let _ = webview.close();
            let _ = state.browser.remove(&tab_id);
            return Err(CommandError::internal(error.to_string()));
        }
    }
    Ok(initial)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn browser_tab_navigate(
    request: BrowserTabNavigateRequest,
    webview: Webview,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    let window = main_window(&webview)?;
    let url = validated_http_url(&request.url)?;
    browser_webview(&window, &state.browser, &request.tab_id)?
        .navigate(url)
        .map_err(|error| CommandError::internal(error.to_string()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn browser_tab_history(
    request: BrowserTabHistoryRequest,
    webview: Webview,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    let window = main_window(&webview)?;
    let script = match request.direction {
        BrowserHistoryDirection::Back => "history.back()",
        BrowserHistoryDirection::Forward => "history.forward()",
    };
    browser_webview(&window, &state.browser, &request.tab_id)?
        .eval(script)
        .map_err(|error| CommandError::internal(error.to_string()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn browser_tab_reload(
    request: BrowserTabRequest,
    webview: Webview,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    let window = main_window(&webview)?;
    browser_webview(&window, &state.browser, &request.tab_id)?
        .reload()
        .map_err(|error| CommandError::internal(error.to_string()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn browser_tab_set_bounds(
    request: BrowserTabBoundsRequest,
    webview: Webview,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    let window = main_window(&webview)?;
    let bounds = constrained_bounds(&window, &request.bounds)?;
    let webview = browser_webview(&window, &state.browser, &request.tab_id)?;
    webview
        .set_bounds(tauri::Rect {
            position: tauri::Position::Logical(LogicalPosition::new(bounds.x, bounds.y)),
            size: tauri::Size::Logical(LogicalSize::new(bounds.width, bounds.height)),
        })
        .map_err(|error| CommandError::internal(error.to_string()))?;
    if request.visible {
        webview.show()
    } else {
        webview.hide()
    }
    .map_err(|error| CommandError::internal(error.to_string()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn browser_tab_close(
    request: BrowserTabRequest,
    webview: Webview,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    let window = main_window(&webview)?;
    let label = state.browser.label(&request.tab_id)?;
    if let Some(webview) = window.get_webview(&label) {
        webview
            .close()
            .map_err(|error| CommandError::internal(error.to_string()))?;
    }
    state
        .browser
        .remove(&request.tab_id)?
        .ok_or_else(|| CommandError::not_found("Browser tab"))?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn browser_tabs_reset(
    webview: Webview,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    let window = main_window(&webview)?;
    for record in state.browser.reset()? {
        if let Some(webview) = window.get_webview(&record.label) {
            let _ = webview.close();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_policy_accepts_only_credential_free_http_urls() {
        assert!(validated_http_url("https://example.com/path?q=1").is_ok());
        assert!(validated_http_url("http://localhost:1420").is_ok());
        assert!(validated_http_url("file:///tmp/example").is_err());
        assert!(validated_http_url("https://user@example.com").is_err());
        assert!(validated_http_url("https://user:secret@example.com").is_err());
        assert!(validated_http_url(&format!(
            "https://example.com/{}",
            "x".repeat(MAX_URL_BYTES)
        ))
        .is_err());
    }

    #[test]
    fn bounds_are_finite_positive_and_clamped_to_the_window() {
        let bounds = BrowserBounds {
            x: 900.0,
            y: 700.0,
            width: 500.0,
            height: 500.0,
        }
        .constrained(1_000.0, 800.0)
        .unwrap();
        assert_eq!(bounds.x, 900.0);
        assert_eq!(bounds.y, 700.0);
        assert_eq!(bounds.width, 100.0);
        assert_eq!(bounds.height, 100.0);
        assert!(BrowserBounds {
            x: f64::NAN,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        }
        .constrained(100.0, 100.0)
        .is_err());
    }

    #[test]
    fn main_capability_is_scoped_to_the_local_webview() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        assert_eq!(capability["webviews"], serde_json::json!(["main"]));
        assert!(capability.get("windows").is_none());
    }

    #[test]
    fn registry_enforces_tab_ids_and_the_global_webview_limit() {
        let manager = BrowserManager::new(PathBuf::from("browser-test-profile"));
        let url = validated_http_url("https://example.com").unwrap();
        let mut tab_ids = Vec::new();
        for _ in 0..MAX_BROWSER_TABS {
            tab_ids.push(manager.reserve(&url).unwrap().tab_id);
        }
        assert!(manager.reserve(&url).is_err());
        assert!(manager.label("not-a-uuid").is_err());

        manager.remove(&tab_ids[0]).unwrap().unwrap();
        assert!(manager.reserve(&url).is_ok());
        assert_eq!(manager.reset().unwrap().len(), MAX_BROWSER_TABS);
    }
}
