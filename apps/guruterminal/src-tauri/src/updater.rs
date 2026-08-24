use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::{
    app::{AppState, CommandError},
    maintenance::MaintenanceBlocker,
    store::{GuruTerminalStore, SqliteStore, StoreError},
};

const UPDATE_SCHEDULE_SCHEMA: u32 = 1;
#[cfg(any(test, all(any(target_os = "macos", windows), not(debug_assertions))))]
const STABLE_UPDATE_ENDPOINT: &str =
    "https://github.com/monarchjuno/guruterminal/releases/latest/download/latest.json";
const SUCCESS_INTERVAL_MS: i64 = 24 * 60 * 60 * 1_000;
const RETRY_DELAYS_MS: [i64; 4] = [
    15 * 60 * 1_000,
    60 * 60 * 1_000,
    6 * 60 * 60 * 1_000,
    24 * 60 * 60 * 1_000,
];

// The plugin leaves updater request timeouts unset by default. Keep both the
// HTTP timeout and a slightly wider operation deadline so a stalled endpoint
// cannot leave the native coordinator (or its maintenance lease) busy forever.
#[cfg(any(test, all(any(target_os = "macos", windows), not(debug_assertions))))]
const UPDATE_PREFLIGHT_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
#[cfg(any(test, all(any(target_os = "macos", windows), not(debug_assertions))))]
const UPDATE_METADATA_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
#[cfg(any(test, all(any(target_os = "macos", windows), not(debug_assertions))))]
const UPDATE_METADATA_OPERATION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(35);
#[cfg(any(test, all(any(target_os = "macos", windows), not(debug_assertions))))]
const UPDATE_PACKAGE_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);
#[cfg(any(test, all(any(target_os = "macos", windows), not(debug_assertions))))]
const UPDATE_PACKAGE_OPERATION_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(31 * 60);

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PersistedUpdateSchedule {
    schema_version: u32,
    failure_count: u32,
    last_attempt_at_ms: Option<i64>,
    last_successful_check_at_ms: Option<i64>,
    next_auto_check_at_ms: i64,
}

impl Default for PersistedUpdateSchedule {
    fn default() -> Self {
        Self {
            schema_version: UPDATE_SCHEDULE_SCHEMA,
            failure_count: 0,
            last_attempt_at_ms: None,
            last_successful_check_at_ms: None,
            next_auto_check_at_ms: 0,
        }
    }
}

impl PersistedUpdateSchedule {
    pub(crate) fn validate(&self) -> Result<(), StoreError> {
        if self.schema_version != UPDATE_SCHEDULE_SCHEMA {
            return Err(StoreError::Invalid(
                "stored update schedule schema is unsupported".into(),
            ));
        }
        if self.failure_count > 4
            || self.next_auto_check_at_ms < 0
            || self.last_attempt_at_ms.is_some_and(|value| value < 0)
            || self
                .last_successful_check_at_ms
                .is_some_and(|value| value < 0)
        {
            return Err(StoreError::Invalid(
                "stored update schedule is invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    #[default]
    Idle,
    Checking,
    Confirming,
    Downloading,
    Installing,
    Restarting,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UpdateOfferDto {
    pub offer_id: String,
    pub version: String,
    pub notes: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UpdateStateDto {
    pub supported: bool,
    pub current_version: String,
    pub phase: UpdatePhase,
    pub offer: Option<UpdateOfferDto>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub last_checked_at_ms: Option<i64>,
    pub next_auto_check_at_ms: Option<i64>,
    pub error: Option<String>,
    pub blockers: Vec<MaintenanceBlocker>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateInstallRequest {
    pub offer_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UpdateInstallResult {
    pub outcome: UpdateInstallOutcome,
    pub blockers: Vec<MaintenanceBlocker>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateInstallOutcome {
    Blocked,
    Cancelled,
}

#[derive(Debug, Default)]
struct UpdateRuntimeState {
    phase: UpdatePhase,
    offer: Option<UpdateOfferDto>,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    error: Option<String>,
    blockers: Vec<MaintenanceBlocker>,
    schedule: PersistedUpdateSchedule,
}

#[derive(Clone)]
pub struct UpdateCoordinator {
    inner: Arc<Mutex<UpdateRuntimeState>>,
    store: Arc<SqliteStore>,
    #[cfg(test)]
    reject_schedule_saves: Arc<std::sync::atomic::AtomicBool>,
}

impl UpdateCoordinator {
    pub fn new(store: Arc<SqliteStore>) -> Result<Self, CommandError> {
        let schedule = store
            .get_update_schedule()
            .map_err(|error| CommandError::internal(error.to_string()))?
            .unwrap_or_default();
        Ok(Self {
            inner: Arc::new(Mutex::new(UpdateRuntimeState {
                schedule,
                ..UpdateRuntimeState::default()
            })),
            store,
            #[cfg(test)]
            reject_schedule_saves: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    fn save_schedule(&self, schedule: &PersistedUpdateSchedule) -> Result<(), StoreError> {
        #[cfg(test)]
        if self
            .reject_schedule_saves
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(StoreError::Invalid(
                "injected update schedule persistence failure".into(),
            ));
        }
        self.store.save_update_schedule(schedule)
    }

    #[cfg(test)]
    fn reject_schedule_saves(&self, reject: bool) {
        self.reject_schedule_saves
            .store(reject, std::sync::atomic::Ordering::SeqCst);
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, UpdateRuntimeState>, CommandError> {
        self.inner
            .lock()
            .map_err(|_| CommandError::internal("update coordinator lock was poisoned"))
    }

    fn snapshot(
        &self,
        supported: bool,
        current_version: String,
    ) -> Result<UpdateStateDto, CommandError> {
        let state = self.lock()?;
        Ok(UpdateStateDto {
            supported,
            current_version,
            phase: state.phase,
            offer: state.offer.clone(),
            downloaded_bytes: state.downloaded_bytes,
            total_bytes: state.total_bytes,
            last_checked_at_ms: state.schedule.last_successful_check_at_ms,
            next_auto_check_at_ms: supported.then_some(state.schedule.next_auto_check_at_ms),
            error: state.error.clone(),
            blockers: state.blockers.clone(),
        })
    }

    fn begin_check(&self, automatic: bool, now_ms: i64) -> Result<bool, CommandError> {
        let mut state = self.lock()?;
        if automatic && now_ms < state.schedule.next_auto_check_at_ms {
            return Ok(false);
        }
        if state.phase != UpdatePhase::Idle {
            if automatic {
                return Ok(false);
            }
            return Err(CommandError::new(
                "update_busy",
                "another update operation is already active",
            ));
        }
        let mut next_schedule = state.schedule.clone();
        next_schedule.last_attempt_at_ms = Some(now_ms);
        // Persist a conservative retry boundary before network I/O. If the app
        // exits mid-check, the next launch will not immediately hammer the feed.
        next_schedule.next_auto_check_at_ms = now_ms + retry_delay_ms(0, now_ms);
        if let Err(error) = self.save_schedule(&next_schedule) {
            state.error = Some("Could not save update scheduling state.".into());
            return Err(CommandError::internal(error.to_string()));
        }
        state.phase = UpdatePhase::Checking;
        state.error = None;
        state.blockers.clear();
        state.downloaded_bytes = 0;
        state.total_bytes = None;
        state.schedule = next_schedule;
        Ok(true)
    }

    fn finish_check(&self, offer: Option<UpdateOfferDto>, now_ms: i64) -> Result<(), CommandError> {
        let mut state = self.lock()?;
        let mut next_schedule = state.schedule.clone();
        next_schedule.failure_count = 0;
        next_schedule.last_successful_check_at_ms = Some(now_ms);
        next_schedule.next_auto_check_at_ms = now_ms + SUCCESS_INTERVAL_MS;
        if let Err(error) = self.save_schedule(&next_schedule) {
            state.phase = UpdatePhase::Idle;
            state.error = Some("Could not save update scheduling state.".into());
            return Err(CommandError::internal(error.to_string()));
        }
        state.phase = UpdatePhase::Idle;
        state.offer = offer;
        state.error = None;
        state.schedule = next_schedule;
        Ok(())
    }

    fn fail_check(&self, now_ms: i64) -> Result<(), CommandError> {
        let mut state = self.lock()?;
        let mut next_schedule = state.schedule.clone();
        next_schedule.failure_count = next_schedule.failure_count.saturating_add(1).min(4);
        next_schedule.next_auto_check_at_ms =
            now_ms + retry_delay_ms(next_schedule.failure_count - 1, now_ms);
        if let Err(error) = self.save_schedule(&next_schedule) {
            state.phase = UpdatePhase::Idle;
            state.error = Some("Could not save update scheduling state.".into());
            return Err(CommandError::internal(error.to_string()));
        }
        state.phase = UpdatePhase::Idle;
        state.error = Some("Could not check for updates. Guru Terminal will retry.".into());
        state.schedule = next_schedule;
        Ok(())
    }

    fn begin_install(&self, offer_id: &str) -> Result<UpdateOfferDto, CommandError> {
        let mut state = self.lock()?;
        if state.phase != UpdatePhase::Idle {
            return Err(CommandError::new(
                "update_busy",
                "another update operation is already active",
            ));
        }
        let offer = state
            .offer
            .as_ref()
            .filter(|offer| offer.offer_id == offer_id)
            .cloned()
            .ok_or_else(|| {
                CommandError::conflict(
                    "the update offer expired or changed; check again before installing",
                )
            })?;
        state.phase = UpdatePhase::Confirming;
        state.error = None;
        state.blockers.clear();
        Ok(offer)
    }

    fn install_blocked(&self, blockers: Vec<MaintenanceBlocker>) {
        if let Ok(mut state) = self.lock() {
            state.phase = UpdatePhase::Idle;
            state.blockers = blockers;
            state.error = None;
        }
    }

    #[cfg_attr(
        not(all(any(target_os = "macos", windows), not(debug_assertions))),
        allow(dead_code)
    )]
    fn install_cancelled(&self) {
        if let Ok(mut state) = self.lock() {
            state.phase = UpdatePhase::Idle;
            state.blockers.clear();
            state.error = None;
        }
    }

    fn install_failed(&self, message: impl Into<String>, invalidate_offer: bool) {
        if let Ok(mut state) = self.lock() {
            state.phase = UpdatePhase::Idle;
            state.downloaded_bytes = 0;
            state.total_bytes = None;
            state.blockers.clear();
            state.error = Some(message.into());
            if invalidate_offer {
                state.offer = None;
            }
        }
    }

    #[cfg_attr(
        not(all(any(target_os = "macos", windows), not(debug_assertions))),
        allow(dead_code)
    )]
    fn start_download(&self) {
        if let Ok(mut state) = self.lock() {
            state.phase = UpdatePhase::Downloading;
            state.downloaded_bytes = 0;
            state.total_bytes = None;
        }
    }

    #[cfg_attr(
        not(all(any(target_os = "macos", windows), not(debug_assertions))),
        allow(dead_code)
    )]
    fn record_download(&self, downloaded_bytes: u64, total_bytes: Option<u64>) {
        if let Ok(mut state) = self.lock() {
            state.downloaded_bytes = downloaded_bytes;
            state.total_bytes = total_bytes;
        }
    }

    #[cfg_attr(
        not(all(any(target_os = "macos", windows), not(debug_assertions))),
        allow(dead_code)
    )]
    fn download_finished(&self) {
        if let Ok(mut state) = self.lock() {
            state.phase = UpdatePhase::Installing;
        }
    }

    #[cfg_attr(
        not(all(any(target_os = "macos", windows), not(debug_assertions))),
        allow(dead_code)
    )]
    fn restarting(&self) {
        if let Ok(mut state) = self.lock() {
            state.phase = UpdatePhase::Restarting;
        }
    }

    fn auto_check_is_due(&self, now_ms: i64) -> bool {
        self.inner
            .lock()
            .map(|state| {
                state.phase == UpdatePhase::Idle && now_ms >= state.schedule.next_auto_check_at_ms
            })
            .unwrap_or(false)
    }
}

fn retry_delay_ms(failure_index: u32, now_ms: i64) -> i64 {
    let base = RETRY_DELAYS_MS[failure_index.min(3) as usize];
    let jitter_window = (base / 10).max(1);
    base + now_ms.rem_euclid(jitter_window)
}

fn current_version(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

fn updater_supported(app: &AppHandle) -> bool {
    #[cfg(all(any(target_os = "macos", windows), not(debug_assertions)))]
    {
        configured(&tauri::Manager::config(app).plugins, &current_version(app))
    }
    #[cfg(not(all(any(target_os = "macos", windows), not(debug_assertions))))]
    {
        let _ = app;
        false
    }
}

#[cfg(any(test, all(any(target_os = "macos", windows), not(debug_assertions))))]
fn expected_rc_endpoints(version: &str) -> Option<Vec<String>> {
    let (base, rc) = version.split_once("-rc.")?;
    let mut base_parts = base.split('.');
    let valid_number = |part: &str| {
        !part.is_empty()
            && part.bytes().all(|byte| byte.is_ascii_digit())
            && (part == "0" || !part.starts_with('0'))
    };
    if !valid_number(base_parts.next()?)
        || !valid_number(base_parts.next()?)
        || !valid_number(base_parts.next()?)
        || base_parts.next().is_some()
        || !valid_number(rc)
        || rc == "0"
    {
        return None;
    }
    let next = rc.parse::<u64>().ok()?.checked_add(1)?;
    Some(vec![
        format!(
            "https://github.com/monarchjuno/guruterminal/releases/download/v{base}-rc.{next}/latest.json"
        ),
        STABLE_UPDATE_ENDPOINT.into(),
    ])
}

#[cfg(any(test, all(any(target_os = "macos", windows), not(debug_assertions))))]
fn expected_update_endpoints(version: &str) -> Option<Vec<String>> {
    if let Some(endpoints) = expected_rc_endpoints(version) {
        return Some(endpoints);
    }
    let mut parts = version.split('.');
    let valid_number = |part: &str| {
        !part.is_empty()
            && part.bytes().all(|byte| byte.is_ascii_digit())
            && (part == "0" || !part.starts_with('0'))
    };
    (valid_number(parts.next()?)
        && valid_number(parts.next()?)
        && valid_number(parts.next()?)
        && parts.next().is_none())
    .then(|| vec![STABLE_UPDATE_ENDPOINT.into()])
}

#[cfg(any(test, all(any(target_os = "macos", windows), not(debug_assertions))))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FeedStatus {
    Absent,
    Available,
    Fail,
}

#[cfg(any(test, all(any(target_os = "macos", windows), not(debug_assertions))))]
fn classify_feed_status(status: u16) -> FeedStatus {
    match status {
        404 => FeedStatus::Absent,
        200..=299 => FeedStatus::Available,
        _ => FeedStatus::Fail,
    }
}

#[cfg(any(test, all(any(target_os = "macos", windows), not(debug_assertions))))]
fn combine_feed_status(statuses: impl IntoIterator<Item = FeedStatus>) -> FeedStatus {
    let mut combined = FeedStatus::Absent;
    for status in statuses {
        match status {
            FeedStatus::Fail => return FeedStatus::Fail,
            FeedStatus::Available => combined = FeedStatus::Available,
            FeedStatus::Absent => {}
        }
    }
    combined
}

#[cfg(any(test, all(any(target_os = "macos", windows), not(debug_assertions))))]
fn trusted_github_update_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.port(), None | Some(443))
        && url
            .host_str()
            .is_some_and(|host| host == "github.com" || host.ends_with(".githubusercontent.com"))
}

#[cfg(any(test, all(any(target_os = "macos", windows), not(debug_assertions))))]
async fn finish_before_deadline<T>(
    deadline: std::time::Duration,
    operation: impl std::future::Future<Output = T>,
) -> Result<T, tokio::time::error::Elapsed> {
    tokio::time::timeout(deadline, operation).await
}

#[cfg(all(any(target_os = "macos", windows), not(debug_assertions)))]
async fn bounded_update_check(
    app: &AppHandle,
) -> Result<Option<tauri_plugin_updater::Update>, CommandError> {
    use tauri_plugin_updater::UpdaterExt;

    let updater = app
        .updater_builder()
        .timeout(UPDATE_METADATA_REQUEST_TIMEOUT)
        .build()
        .map_err(|_| CommandError::unavailable("updater"))?;
    finish_before_deadline(UPDATE_METADATA_OPERATION_DEADLINE, updater.check())
        .await
        .map_err(|_| CommandError::new("update_check_failed", "update check timed out"))?
        .map_err(|_| CommandError::new("update_check_failed", "update check failed"))
}

#[cfg(all(any(target_os = "macos", windows), not(debug_assertions)))]
async fn configured_feed_is_absent(app: &AppHandle) -> Result<bool, CommandError> {
    let version = current_version(app);
    let Some(expected_endpoints) = expected_rc_endpoints(&version) else {
        return Ok(false);
    };
    let configured_endpoints = tauri::Manager::config(app)
        .plugins
        .0
        .get("updater")
        .and_then(|updater| updater.get("endpoints"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| CommandError::new("update_check_failed", "update check failed"))?;
    if configured_endpoints.len() != expected_endpoints.len()
        || configured_endpoints
            .iter()
            .zip(&expected_endpoints)
            .any(|(actual, expected)| actual.as_str() != Some(expected.as_str()))
    {
        return Err(CommandError::new(
            "update_check_failed",
            "update check failed",
        ));
    }

    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many updater feed redirects")
            } else if trusted_github_update_url(attempt.url()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .timeout(UPDATE_PREFLIGHT_REQUEST_TIMEOUT)
        .build()
        .map_err(|_| CommandError::new("update_check_failed", "update check failed"))?;

    let mut statuses = Vec::with_capacity(expected_endpoints.len());
    for endpoint in expected_endpoints {
        let response = client
            .get(endpoint)
            .header(reqwest::header::RANGE, "bytes=0-0")
            .send()
            .await
            .map_err(|_| CommandError::new("update_check_failed", "update check failed"))?;
        statuses.push(classify_feed_status(response.status().as_u16()));
    }
    match combine_feed_status(statuses) {
        FeedStatus::Absent => Ok(true),
        FeedStatus::Available => Ok(false),
        FeedStatus::Fail => Err(CommandError::new(
            "update_check_failed",
            "update check failed",
        )),
    }
}

#[cfg(any(test, all(any(target_os = "macos", windows), not(debug_assertions))))]
pub(crate) fn configured(
    plugins: &tauri::utils::config::PluginConfig,
    current_version: &str,
) -> bool {
    let Some(config) = plugins
        .0
        .get("updater")
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    if !config
        .get("pubkey")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|key| !key.trim().is_empty())
    {
        return false;
    }
    let Some(endpoints) = config
        .get("endpoints")
        .and_then(serde_json::Value::as_array)
    else {
        return false;
    };
    let Some(expected) = expected_update_endpoints(current_version) else {
        return false;
    };
    endpoints.len() == expected.len()
        && endpoints
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.as_str() == Some(expected.as_str()))
}

#[tauri::command(rename_all = "snake_case")]
pub fn update_status(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UpdateStateDto, CommandError> {
    state
        .updates
        .snapshot(updater_supported(&app), current_version(&app))
}

async fn check_for_updates(
    app: &AppHandle,
    coordinator: &UpdateCoordinator,
    automatic: bool,
) -> Result<UpdateStateDto, CommandError> {
    let supported = updater_supported(app);
    if !supported {
        return coordinator.snapshot(false, current_version(app));
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    if !coordinator.begin_check(automatic, now_ms)? {
        return coordinator.snapshot(true, current_version(app));
    }

    #[cfg(all(any(target_os = "macos", windows), not(debug_assertions)))]
    let checked = async {
        if configured_feed_is_absent(app).await? {
            return Ok(None);
        }
        let update = bounded_update_check(app).await?;
        Ok::<_, CommandError>(update.map(|update| UpdateOfferDto {
            offer_id: uuid::Uuid::new_v4().to_string(),
            version: update.version,
            notes: update.body,
            published_at: update.date.map(|date| date.to_string()),
        }))
    }
    .await;

    #[cfg(not(all(any(target_os = "macos", windows), not(debug_assertions))))]
    let checked: Result<Option<UpdateOfferDto>, CommandError> = Ok(None);

    match checked {
        Ok(offer) => {
            coordinator.finish_check(offer, chrono::Utc::now().timestamp_millis())?;
            coordinator.snapshot(true, current_version(app))
        }
        Err(error) => match coordinator.fail_check(chrono::Utc::now().timestamp_millis()) {
            Ok(()) => Err(error),
            Err(persistence_error) => Err(persistence_error),
        },
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn update_check(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UpdateStateDto, CommandError> {
    check_for_updates(&app, &state.updates, false).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn update_install(
    app: AppHandle,
    state: State<'_, AppState>,
    request: UpdateInstallRequest,
) -> Result<UpdateInstallResult, CommandError> {
    if uuid::Uuid::parse_str(&request.offer_id).is_err() {
        return Err(CommandError::invalid("update offer is invalid"));
    }
    if !updater_supported(&app) {
        return Err(CommandError::unavailable("updater"));
    }

    let offer = state.updates.begin_install(&request.offer_id)?;
    let maintenance = match state.maintenance.begin_update() {
        Ok(maintenance) => maintenance,
        Err(blockers) => {
            state.updates.install_blocked(blockers.clone());
            return Ok(UpdateInstallResult {
                outcome: UpdateInstallOutcome::Blocked,
                blockers,
            });
        }
    };

    #[cfg(all(any(target_os = "macos", windows), not(debug_assertions)))]
    {
        use rfd::{AsyncMessageDialog, MessageButtons, MessageDialogResult, MessageLevel};

        let confirmed = AsyncMessageDialog::new()
            .set_level(MessageLevel::Warning)
            .set_title("Install Guru Terminal update")
            .set_description(format!(
                "Update Guru Terminal from {} to {}?\n\nActive work has been checked. The app will download the signed update, install it, and restart immediately.",
                current_version(&app), offer.version
            ))
            .set_buttons(MessageButtons::YesNo)
            .show()
            .await
            == MessageDialogResult::Yes;
        if !confirmed {
            drop(maintenance);
            state.updates.install_cancelled();
            return Ok(UpdateInstallResult {
                outcome: UpdateInstallOutcome::Cancelled,
                blockers: Vec::new(),
            });
        }

        let update = match async {
            if configured_feed_is_absent(&app).await? {
                return Err(CommandError::not_found("update"));
            }
            bounded_update_check(&app)
                .await?
                .ok_or_else(|| CommandError::not_found("update"))
        }
        .await
        {
            Ok(update) if update.version == offer.version => update,
            Ok(_) => {
                drop(maintenance);
                state.updates.install_failed(
                    "The available update changed. Check again before installing.",
                    true,
                );
                return Err(CommandError::conflict(
                    "the available update changed; check again before installing",
                ));
            }
            Err(error) => {
                drop(maintenance);
                state
                    .updates
                    .install_failed("Could not verify the update. Try again.", false);
                return Err(error);
            }
        };

        state.updates.start_download();
        let progress = state.updates.clone();
        let mut downloaded_bytes = 0_u64;
        let mut update = update;
        update.timeout = Some(UPDATE_PACKAGE_REQUEST_TIMEOUT);
        let package = match finish_before_deadline(
            UPDATE_PACKAGE_OPERATION_DEADLINE,
            update.download(
                move |chunk_bytes, total_bytes| {
                    downloaded_bytes = downloaded_bytes.saturating_add(chunk_bytes as u64);
                    progress.record_download(downloaded_bytes, total_bytes);
                },
                || {},
            ),
        )
        .await
        {
            Ok(Ok(package)) => package,
            Ok(Err(_)) => {
                drop(maintenance);
                state
                    .updates
                    .install_failed("Could not download the update. Try again.", false);
                return Err(CommandError::new(
                    "update_download_failed",
                    "update download failed",
                ));
            }
            Err(_) => {
                drop(maintenance);
                state
                    .updates
                    .install_failed("The update download timed out. Try again.", false);
                return Err(CommandError::new(
                    "update_download_failed",
                    "update download timed out",
                ));
            }
        };
        state.updates.download_finished();
        if update.install(package).is_err() {
            drop(maintenance);
            state
                .updates
                .install_failed("Could not install the update. Try again.", false);
            return Err(CommandError::new(
                "update_install_failed",
                "update installation failed",
            ));
        }
        state.updates.restarting();
        std::mem::forget(maintenance);
        app.restart();
    }

    #[cfg(not(all(any(target_os = "macos", windows), not(debug_assertions))))]
    {
        let _ = (app, offer, maintenance);
        state
            .updates
            .install_failed("Automatic updates are unavailable in this build.", false);
        Err(CommandError::unavailable("updater"))
    }
}

pub fn start_auto_update_loop(app: AppHandle, coordinator: UpdateCoordinator) {
    if !updater_supported(&app) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            let now_ms = chrono::Utc::now().timestamp_millis();
            if coordinator.auto_check_is_due(now_ms) {
                let _ = check_for_updates(&app, &coordinator, true).await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(15 * 60)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use super::*;
    use tauri::utils::config::PluginConfig;

    #[test]
    fn install_request_accepts_only_an_opaque_offer_id() {
        assert!(
            serde_json::from_value::<UpdateInstallRequest>(serde_json::json!({
                "offer_id": uuid::Uuid::new_v4().to_string()
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<UpdateInstallRequest>(serde_json::json!({
                "offer_id": uuid::Uuid::new_v4().to_string(),
                "url": "https://attacker.invalid/update"
            }))
            .is_err()
        );
    }

    #[test]
    fn updater_requires_the_exact_signed_feed_config_for_the_current_version() {
        assert!(!configured(&PluginConfig::default(), "0.0.1"));

        let mut plugins = HashMap::new();
        plugins.insert("updater".to_owned(), serde_json::Value::Null);
        assert!(!configured(&PluginConfig(plugins.clone()), "0.0.1"));

        plugins.insert(
            "updater".to_owned(),
            serde_json::json!({"windows": {"installMode": "passive"}}),
        );
        assert!(!configured(&PluginConfig(plugins.clone()), "0.0.1"));

        plugins.insert(
            "updater".to_owned(),
            serde_json::json!({
                "pubkey": "test-public-key",
                "endpoints": [STABLE_UPDATE_ENDPOINT]
            }),
        );
        assert!(configured(&PluginConfig(plugins.clone()), "0.0.1"));
        assert!(!configured(&PluginConfig(plugins.clone()), "0.0.1-rc.1"));
        assert!(!configured(&PluginConfig(plugins), "0.0.1-beta.1"));
    }

    #[test]
    fn rc_endpoints_include_the_exact_next_rc_then_stable() {
        assert_eq!(
            expected_rc_endpoints("1.0.0-rc.1").unwrap(),
            vec![
                "https://github.com/monarchjuno/guruterminal/releases/download/v1.0.0-rc.2/latest.json",
                "https://github.com/monarchjuno/guruterminal/releases/latest/download/latest.json",
            ]
        );
        assert!(expected_rc_endpoints("1.0.0").is_none());
        assert!(expected_rc_endpoints("1.0.0-beta.1").is_none());
        assert!(expected_rc_endpoints("1.0.0-rc.0").is_none());
        assert!(expected_rc_endpoints("1.0.0-rc.01").is_none());
        assert!(expected_rc_endpoints("1.0.0-rc.1+build").is_none());
    }

    #[test]
    fn only_404_is_absent_and_other_failures_are_closed() {
        assert_eq!(classify_feed_status(404), FeedStatus::Absent);
        assert_eq!(classify_feed_status(200), FeedStatus::Available);
        assert_eq!(classify_feed_status(206), FeedStatus::Available);
        assert_eq!(classify_feed_status(403), FeedStatus::Fail);
        assert_eq!(classify_feed_status(500), FeedStatus::Fail);
        assert_eq!(
            combine_feed_status([FeedStatus::Absent, FeedStatus::Absent]),
            FeedStatus::Absent
        );
        assert_eq!(
            combine_feed_status([FeedStatus::Absent, FeedStatus::Available]),
            FeedStatus::Available
        );
        assert_eq!(
            combine_feed_status([FeedStatus::Available, FeedStatus::Fail]),
            FeedStatus::Fail
        );
    }

    #[test]
    fn updater_feed_redirects_stay_on_trusted_https_github_hosts() {
        assert!(trusted_github_update_url(
            &"https://github.com/monarchjuno/guruterminal/releases/download/v1.0.0-rc.2/latest.json"
                .parse()
                .unwrap()
        ));
        assert!(trusted_github_update_url(
            &"https://release-assets.githubusercontent.com/path?token=opaque"
                .parse()
                .unwrap()
        ));
        assert!(!trusted_github_update_url(
            &"https://githubusercontent.com.evil.test/path"
                .parse()
                .unwrap()
        ));
        assert!(!trusted_github_update_url(
            &"http://github.com/path".parse().unwrap()
        ));
        assert!(!trusted_github_update_url(
            &"https://user@github.com/path".parse().unwrap()
        ));
    }

    #[test]
    fn updater_network_operations_have_finite_layered_timeouts() {
        assert!(UPDATE_PREFLIGHT_REQUEST_TIMEOUT > std::time::Duration::ZERO);
        assert!(UPDATE_METADATA_REQUEST_TIMEOUT > std::time::Duration::ZERO);
        assert!(UPDATE_METADATA_OPERATION_DEADLINE > UPDATE_METADATA_REQUEST_TIMEOUT);
        assert!(UPDATE_PACKAGE_REQUEST_TIMEOUT > UPDATE_METADATA_REQUEST_TIMEOUT);
        assert!(UPDATE_PACKAGE_OPERATION_DEADLINE > UPDATE_PACKAGE_REQUEST_TIMEOUT);
    }

    #[tokio::test]
    async fn stalled_update_operation_hits_the_native_deadline() {
        let result =
            finish_before_deadline(std::time::Duration::ZERO, std::future::pending::<()>()).await;

        assert!(result.is_err());
    }

    #[test]
    fn native_schedule_persists_success_and_failure_backoff() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let coordinator = UpdateCoordinator::new(store.clone()).unwrap();
        let now = 1_800_000_000_000_i64;

        assert!(coordinator.begin_check(true, now).unwrap());
        coordinator.fail_check(now + 1).unwrap();
        let retry_at = store
            .get_update_schedule()
            .unwrap()
            .unwrap()
            .next_auto_check_at_ms;
        assert!(retry_at >= now + RETRY_DELAYS_MS[0]);

        let reloaded = UpdateCoordinator::new(store.clone()).unwrap();
        assert!(!reloaded.begin_check(true, now + 2).unwrap());
        assert!(reloaded.begin_check(false, now + 2).unwrap());
        reloaded.finish_check(None, now + 3).unwrap();

        let schedule = store.get_update_schedule().unwrap().unwrap();
        assert_eq!(schedule.failure_count, 0);
        assert_eq!(schedule.last_successful_check_at_ms, Some(now + 3));
        assert_eq!(
            schedule.next_auto_check_at_ms,
            now + 3 + SUCCESS_INTERVAL_MS
        );
    }

    #[test]
    fn schedule_persistence_failure_does_not_leave_checking_stuck() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let coordinator = UpdateCoordinator::new(store).unwrap();

        let error = coordinator.begin_check(false, -1).unwrap_err();
        assert_eq!(error.code, "internal");
        let state = coordinator.lock().unwrap();
        assert_eq!(state.phase, UpdatePhase::Idle);
        assert_eq!(state.schedule, PersistedUpdateSchedule::default());
        assert!(state.error.is_some());
    }

    #[test]
    fn successful_check_is_not_visible_when_schedule_commit_fails() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let coordinator = UpdateCoordinator::new(store.clone()).unwrap();
        let now = 1_800_000_000_000_i64;
        coordinator.begin_check(false, now).unwrap();
        let committed_before = store.get_update_schedule().unwrap().unwrap();
        coordinator.reject_schedule_saves(true);

        let error = coordinator
            .finish_check(
                Some(UpdateOfferDto {
                    offer_id: uuid::Uuid::new_v4().to_string(),
                    version: "1.0.1".into(),
                    notes: None,
                    published_at: None,
                }),
                now + 1,
            )
            .unwrap_err();

        assert_eq!(error.code, "internal");
        let state = coordinator.lock().unwrap();
        assert_eq!(state.phase, UpdatePhase::Idle);
        assert_eq!(state.offer, None);
        assert_eq!(state.schedule, committed_before);
        assert!(state.error.is_some());
        assert_eq!(
            store.get_update_schedule().unwrap().unwrap(),
            committed_before
        );
    }

    #[test]
    fn failed_check_backoff_is_rolled_back_when_schedule_commit_fails() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let coordinator = UpdateCoordinator::new(store.clone()).unwrap();
        let now = 1_800_000_000_000_i64;
        coordinator.begin_check(false, now).unwrap();
        let committed_before = store.get_update_schedule().unwrap().unwrap();
        coordinator.reject_schedule_saves(true);

        let error = coordinator.fail_check(now + 1).unwrap_err();

        assert_eq!(error.code, "internal");
        let state = coordinator.lock().unwrap();
        assert_eq!(state.phase, UpdatePhase::Idle);
        assert_eq!(state.schedule, committed_before);
        assert!(state.error.is_some());
        assert_eq!(
            store.get_update_schedule().unwrap().unwrap(),
            committed_before
        );
    }
}
