use std::{
    collections::{BTreeMap, HashMap},
    env,
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
#[cfg(not(windows))]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use tauri::{AppHandle, Manager};

#[cfg(windows)]
use crate::windows_fs::{filesystem_identity, open_regular_no_reparse};
use crate::{
    artifact_trust::{ensure_private_directory, ensure_private_regular_file},
    browser::BrowserManager,
    chat_control::{AcceptedChatControl, ChatControlHandle, ChatControlKind},
    compute::ComputeArtifacts,
    deletion,
    finance_data::FinanceDataService,
    maintenance::MaintenanceCoordinator,
    pi::PiLaunchConfig,
    pi_execution::PiExecutionConfig,
    process_lease::{prepare_lease_directory, recover_orphaned_processes},
    provider_connection::ProviderModelDiscoveryCache,
    run_coordinator::{
        PendingMemoryWrite, RunCoordinator, RunKind, RunRegistration, RunSpec, RunTarget,
    },
    runtime::GuruTerminalRuntime,
    secure_delete::SecureDeletionRoot,
    settings::{
        catalog_view, provider_credential_from_environment, ConfiguredModel, ModelCatalog,
        ModelCatalogView, ModelVisibility,
    },
    store::{GuruTerminalStore, SqliteStore},
    support_coordinator::ProviderSupportCoordinator,
    updater::UpdateCoordinator,
};

const APP_INSTANCE_CONFLICT_CODE: &str = "app_instance_conflict";
#[cfg(any(not(feature = "e2e"), test))]
const PRODUCTION_APP_IDENTIFIER: &str = crate::DEVELOPMENT_APP_IDENTIFIER;
#[cfg(any(test, feature = "e2e"))]
pub(crate) const LIVE_PI_AGENT_DATA_DIR_ENV: &str = "GURUTERMINAL_LIVE_PI_AGENT_DATA_DIR";

#[derive(Clone, Debug, Serialize)]
pub struct CommandError {
    pub code: String,
    pub message: String,
}

impl CommandError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn unavailable(component: &str) -> Self {
        Self::new(
            format!("{component}_unavailable"),
            format!("{component} is not available in this Guru Terminal build"),
        )
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalid_request", message)
    }

    pub fn not_found(kind: &str) -> Self {
        Self::new("not_found", format!("{kind} was not found"))
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new("conflict", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message)
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for CommandError {}

#[derive(Clone)]
pub struct PiArtifacts {
    pub executable: PathBuf,
    pub runtime_dir: PathBuf,
    pub extension: PathBuf,
    pub provider_extension: PathBuf,
    pub system_prompt: PathBuf,
    pub provider: String,
    pub model: String,
    pub thinking_level: String,
    pub run_options: std::collections::BTreeMap<String, String>,
    pub provider_credential: Option<(String, String)>,
}

impl PiArtifacts {
    pub fn launch_config(
        &self,
        app_data_dir: &Path,
        guru_id: &str,
        run_id: &str,
        working_dir: PathBuf,
        broker_socket: PathBuf,
        broker_token: String,
    ) -> PiLaunchConfig {
        PiLaunchConfig {
            executable: self.executable.clone(),
            runtime_dir: self.runtime_dir.clone(),
            extension: self.extension.clone(),
            system_prompt: self.system_prompt.clone(),
            // Pi authentication and provider settings are app-scoped. Chat
            // supplies a stable, session-private CWD; run-private capability
            // files remain isolated below the separate private run directory.
            agent_data_dir: app_data_dir.join("pi"),
            working_dir,
            private_run_dir: app_data_dir.join("runs").join(guru_id).join(run_id),
            lease_dir: app_data_dir.join("process-leases"),
            broker_socket,
            broker_token,
            provider: self.provider.clone(),
            model: self.model.clone(),
            thinking_level: self.thinking_level.clone(),
            run_options: self.run_options.clone(),
            provider_credential: self.provider_credential.clone(),
            host_context: None,
            skill_files: Vec::new(),
            session: None,
        }
    }
}

#[derive(Clone)]
pub struct AppArtifacts {
    pub app_data_dir: PathBuf,
    pub broker_dir: PathBuf,
    pub connector_config_dir: PathBuf,
    pub process_lease_dir: PathBuf,
    pub deletion_root: Arc<SecureDeletionRoot>,
    pub pi: Option<PiArtifacts>,
    pub finance_executable: Option<PathBuf>,
    pub compute: Option<ComputeArtifacts>,
    pub mcp_runtimes: BTreeMap<String, crate::mcp::BundledMcpRuntime>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum QuarantineSource {
    Deletion,
    MemoryWrite,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GuruAvailability {
    Available,
    RecoveryRequired {
        reason: GuruRecoveryReason,
        action: GuruRecoveryAction,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuruRecoveryAction {
    RecoverMemory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuruRecoveryReason {
    InterruptedMemoryUpdate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GuruAccess {
    Visible(GuruAvailability),
    Hidden,
}

struct AppInstanceLock {
    _file: File,
}

impl AppInstanceLock {
    fn acquire(app_data_dir: &Path) -> Result<Self, CommandError> {
        let lock_path = app_data_dir.join("guruterminal.instance.lock");
        #[cfg(windows)]
        if matches!(std::fs::symlink_metadata(&lock_path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        {
            ensure_private_regular_file(&lock_path)
                .map_err(|error| CommandError::internal(error.to_string()))?;
        }
        #[cfg(not(windows))]
        ensure_private_regular_file(&lock_path)
            .map_err(|error| CommandError::internal(error.to_string()))?;
        #[cfg(windows)]
        let file = {
            let file = open_regular_no_reparse(&lock_path)
                .map_err(|error| CommandError::internal(error.to_string()))?;
            let reopened = open_regular_no_reparse(&lock_path)
                .map_err(|error| CommandError::internal(error.to_string()))?;
            let identity = filesystem_identity(&file)
                .map_err(|error| CommandError::internal(error.to_string()))?;
            let reopened_identity = filesystem_identity(&reopened)
                .map_err(|error| CommandError::internal(error.to_string()))?;
            if identity != reopened_identity {
                return Err(CommandError::internal(
                    "instance lock path changed while opening",
                ));
            }
            file
        };
        #[cfg(not(windows))]
        let file = {
            let mut options = OpenOptions::new();
            options.truncate(false).read(true).write(true);
            #[cfg(unix)]
            options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
            options
                .open(lock_path)
                .map_err(|error| CommandError::internal(error.to_string()))?
        };
        file.try_lock_exclusive().map_err(|error| {
            CommandError::new(
                APP_INSTANCE_CONFLICT_CODE,
                format!("another Guru Terminal instance already owns this app data ({error})"),
            )
        })?;
        Ok(Self { _file: file })
    }
}

pub(crate) fn is_app_instance_conflict(error: &CommandError) -> bool {
    error.code == APP_INSTANCE_CONFLICT_CODE
}

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<SqliteStore>,
    pub runtime: Option<Arc<GuruTerminalRuntime>>,
    pub artifacts: Arc<AppArtifacts>,
    pub(crate) mcp_pool: crate::mcp_pool::McpProcessPool,
    pub finance_data: Arc<FinanceDataService>,
    pub run_coordinator: RunCoordinator,
    pub provider_support: ProviderSupportCoordinator,
    pub(crate) provider_model_cache: ProviderModelDiscoveryCache,
    pub maintenance: MaintenanceCoordinator,
    pub updates: UpdateCoordinator,
    pub browser: BrowserManager,
    quarantined_gurus: Arc<RwLock<HashMap<String, HashMap<QuarantineSource, String>>>>,
    _instance_lock: Arc<AppInstanceLock>,
    model_catalog: Arc<RwLock<ModelCatalog>>,
    model_visibility: Arc<RwLock<ModelVisibility>>,
    pub(crate) fresh_install: bool,
}

impl AppState {
    pub fn initialize(app: &AppHandle) -> Result<Self, CommandError> {
        #[cfg(not(feature = "e2e"))]
        let platform_app_data_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| CommandError::internal(error.to_string()))?;

        #[cfg(feature = "e2e")]
        let requested_app_data_dir = {
            if app.config().identifier != crate::E2E_APP_IDENTIFIER {
                return Err(CommandError::internal(
                    "the E2E build requires the isolated E2E app identifier",
                ));
            }
            e2e_app_data_dir(env::var_os("GURUTERMINAL_E2E_APP_DATA_DIR").map(PathBuf::from))?
        };
        #[cfg(not(feature = "e2e"))]
        let requested_app_data_dir =
            app_data_dir_for_build(platform_app_data_dir, app.config().identifier.as_str());

        let app_data_dir = pin_app_data_dir(&requested_app_data_dir)?;
        let instance_lock = Arc::new(AppInstanceLock::acquire(&app_data_dir)?);

        let process_lease_dir = app_data_dir.join("process-leases");
        ensure_private_directory(&process_lease_dir)
            .map_err(|error| CommandError::internal(error.to_string()))?;
        prepare_lease_directory(&process_lease_dir)
            .and_then(|()| recover_orphaned_processes(&process_lease_dir))
            .map_err(|error| CommandError::internal(error.to_string()))?;

        for child in [
            "browser-profile",
            "connectors",
            "gurus",
            "mcp-pool",
            "pi",
            "runs",
        ] {
            ensure_private_directory(&app_data_dir.join(child))
                .map_err(|error| CommandError::internal(error.to_string()))?;
        }
        let mcp_pool = crate::mcp_pool::McpProcessPool::prepare(app_data_dir.join("mcp-pool"))
            .map_err(CommandError::internal)?;
        let deletion_root = Arc::new(SecureDeletionRoot::open(&app_data_dir)?);
        crate::run_scratch::sweep_stale_runs(&deletion_root)?;
        let broker_dir = transient_broker_directory();
        ensure_private_directory(&broker_dir)
            .map_err(|error| CommandError::internal(error.to_string()))?;

        let database_path = app_data_dir.join("guruterminal.sqlite3");
        let (store, fresh_install) = open_app_store(&database_path)?;
        let deletion_recovery = deletion::recover(&store, &deletion_root)?;
        let memory_finalization_recovery =
            crate::commands::memory_write::interrupted_memory_finalization_quarantines(&store)?;
        let model_catalog = store
            .get_model_catalog()
            .map_err(|error| CommandError::internal(error.to_string()))?
            .unwrap_or_default();
        let model_visibility = store
            .get_model_visibility()
            .map_err(|error| CommandError::internal(error.to_string()))?
            .unwrap_or_default();
        let resource_dir = app.path().resource_dir().ok();
        let executable_dir = env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf));
        #[cfg(debug_assertions)]
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

        let runtime_candidates = executable_dir
            .iter()
            .flat_map(|directory| {
                [
                    directory.join(platform_binary("guruterminal-core")),
                    directory.join(target_binary("guruterminal-core")),
                ]
            })
            .collect::<Vec<_>>();
        #[cfg(debug_assertions)]
        let runtime_candidates = {
            let mut candidates = runtime_candidates;
            candidates.extend([
                manifest_dir
                    .join("binaries")
                    .join(target_binary("guruterminal-core")),
                local_debug_core_binary(&manifest_dir),
            ]);
            candidates
        };
        let runtime_path = resolve_file(runtime_candidates);
        let runtime = runtime_path
            .and_then(|path| GuruTerminalRuntime::new(path).ok())
            .map(Arc::new);

        let pi_executable_candidates = resource_dir
            .iter()
            .map(|directory| {
                directory
                    .join("pi-runtime")
                    .join(platform_binary("guruterminal-pi"))
            })
            .collect::<Vec<_>>();
        #[cfg(debug_assertions)]
        let pi_executable_candidates = {
            let mut candidates = pi_executable_candidates;
            candidates.extend([manifest_dir
                .join("resources/pi-runtime")
                .join(platform_binary("guruterminal-pi"))]);
            candidates
        };
        let pi_executable = resolve_file(pi_executable_candidates);

        let pi_runtime_candidates = resource_dir
            .iter()
            .map(|directory| directory.join("pi-runtime"))
            .collect::<Vec<_>>();
        #[cfg(debug_assertions)]
        let pi_runtime_candidates = {
            let mut candidates = pi_runtime_candidates;
            candidates.push(manifest_dir.join("resources/pi-runtime"));
            candidates
        };
        let pi_runtime_dir = resolve_directory(pi_runtime_candidates);
        let mcp_runtimes = pi_runtime_dir
            .as_deref()
            .map(|root| crate::mcp::discover_bundled_runtimes(root, &process_lease_dir))
            .unwrap_or_default();

        let agent_candidates = resource_dir
            .iter()
            .map(|directory| directory.join("guruterminal-agent"))
            .collect::<Vec<_>>();
        #[cfg(debug_assertions)]
        let agent_candidates = {
            let mut candidates = agent_candidates;
            candidates.push(manifest_dir.join("../agent"));
            candidates
        };
        let agent_resource = resolve_directory(agent_candidates);
        let pi = resolve_pi_artifacts(pi_executable, pi_runtime_dir, agent_resource);

        let finance_candidates = resource_dir
            .iter()
            .map(|directory| {
                directory
                    .join("pi-runtime/finance-worker")
                    .join(platform_binary("guruterminal-finance"))
            })
            .collect::<Vec<_>>();
        #[cfg(debug_assertions)]
        let finance_candidates = {
            let mut candidates = finance_candidates;
            candidates.extend(local_debug_finance_candidates(&manifest_dir));
            candidates
        };
        let finance_executable = resolve_file(finance_candidates);

        let compute_runtime_candidates = resource_dir
            .iter()
            .map(|directory| directory.join("pi-runtime/compute-worker"))
            .collect::<Vec<_>>();
        #[cfg(debug_assertions)]
        let compute_runtime_candidates = {
            let mut candidates = compute_runtime_candidates;
            candidates.push(manifest_dir.join("resources/pi-runtime/compute-worker"));
            candidates
        };
        let compute = resolve_directory(compute_runtime_candidates).and_then(|runtime_dir| {
            let executable = runtime_dir.join(platform_binary("guruterminal-compute"));
            let bootstrap = runtime_dir.join("bootstrap.mjs");
            let javascript_host = runtime_dir.join("javascript-host.mjs");
            if executable.is_file() && bootstrap.is_file() && javascript_host.is_file() {
                Some(ComputeArtifacts {
                    executable,
                    runtime_dir,
                    bootstrap,
                    lease_dir: process_lease_dir.clone(),
                })
            } else {
                None
            }
        });

        let finance_data = Arc::new(
            FinanceDataService::new().map_err(|error| CommandError::internal(error.to_string()))?,
        );
        let store = Arc::new(store);
        let maintenance = MaintenanceCoordinator::default();
        let updates = UpdateCoordinator::new(store.clone())?;
        let mut quarantined_gurus = deletion_recovery
            .quarantined_gurus
            .into_iter()
            .map(|(guru_id, reason)| {
                (
                    guru_id,
                    HashMap::from([(QuarantineSource::Deletion, reason)]),
                )
            })
            .collect::<HashMap<_, _>>();
        for (guru_id, reason) in memory_finalization_recovery {
            quarantined_gurus
                .entry(guru_id)
                .or_default()
                .insert(QuarantineSource::MemoryWrite, reason);
        }
        Ok(Self {
            store,
            runtime,
            artifacts: Arc::new(AppArtifacts {
                broker_dir,
                connector_config_dir: app_data_dir.join("connectors"),
                process_lease_dir,
                deletion_root,
                app_data_dir: app_data_dir.clone(),
                pi,
                finance_executable,
                compute,
                mcp_runtimes,
            }),
            mcp_pool,
            finance_data,
            run_coordinator: RunCoordinator::new(maintenance.clone()),
            provider_support: ProviderSupportCoordinator::new(maintenance.clone()),
            provider_model_cache: ProviderModelDiscoveryCache::default(),
            maintenance,
            updates,
            browser: BrowserManager::new(app_data_dir.join("browser-profile")),
            quarantined_gurus: Arc::new(RwLock::new(quarantined_gurus)),
            _instance_lock: instance_lock,
            model_catalog: Arc::new(RwLock::new(model_catalog)),
            model_visibility: Arc::new(RwLock::new(model_visibility)),
            fresh_install,
        })
    }

    pub fn runtime(&self) -> Result<Arc<GuruTerminalRuntime>, CommandError> {
        self.runtime
            .clone()
            .ok_or_else(|| CommandError::unavailable("guru_runtime"))
    }

    pub fn pi_execution(
        &self,
        model_profile_id: &str,
        thinking_level: &str,
        run_options: &std::collections::BTreeMap<String, String>,
    ) -> Result<PiExecutionConfig, CommandError> {
        let model = self.model_catalog()?.resolve(model_profile_id)?;
        if !self.model_visibility()?.is_visible(&model.id) {
            return Err(CommandError::invalid(
                "the selected Pi model is hidden from Chat",
            ));
        }
        let mut pi = self
            .artifacts
            .pi
            .clone()
            .ok_or_else(|| CommandError::unavailable("pi"))?;
        pi.provider_credential = provider_credential_from_environment(&model.provider);
        PiExecutionConfig::new(pi, model, thinking_level, run_options)
    }

    pub fn pi_agent_data_dir(&self) -> Result<PathBuf, CommandError> {
        #[cfg(any(test, feature = "e2e"))]
        if let Some(path) = live_pi_agent_data_dir_override()? {
            return Ok(path);
        }
        Ok(self.artifacts.app_data_dir.join("pi"))
    }

    pub fn model_catalog(&self) -> Result<ModelCatalog, CommandError> {
        self.model_catalog
            .read()
            .map_err(|_| CommandError::internal("model settings lock was poisoned"))
            .map(|catalog| catalog.clone())
    }

    pub fn model_visibility(&self) -> Result<ModelVisibility, CommandError> {
        self.model_visibility
            .read()
            .map_err(|_| CommandError::internal("model visibility lock was poisoned"))
            .map(|visibility| visibility.clone())
    }

    pub fn model_catalog_view(&self) -> Result<ModelCatalogView, CommandError> {
        catalog_view(
            &self.model_catalog()?,
            &self.pi_agent_data_dir()?,
            &self.model_visibility()?,
        )
    }

    pub fn set_model_visible(
        &self,
        model_profile_id: &str,
        visible: bool,
    ) -> Result<(), CommandError> {
        self.model_catalog()?.resolve(model_profile_id)?;
        let mut current = self
            .model_visibility
            .write()
            .map_err(|_| CommandError::internal("model visibility lock was poisoned"))?;
        let mut next = current.clone();
        next.set_visible(model_profile_id, visible);
        next.validate()?;
        self.store
            .save_model_visibility(&next)
            .map_err(|error| CommandError::internal(error.to_string()))?;
        *current = next;
        Ok(())
    }

    pub fn set_model_catalog(&self, catalog: ModelCatalog) -> Result<(), CommandError> {
        catalog.validate()?;
        self.store
            .save_model_catalog(&catalog)
            .map_err(|error| CommandError::internal(error.to_string()))?;
        *self
            .model_catalog
            .write()
            .map_err(|_| CommandError::internal("model settings lock was poisoned"))? = catalog;
        Ok(())
    }

    pub fn replace_provider_models(
        &self,
        provider: &str,
        models: Vec<ConfiguredModel>,
    ) -> Result<(), CommandError> {
        let mut catalog = self.model_catalog()?;
        catalog.models.retain(|model| model.provider != provider);
        catalog.models.extend(models);
        catalog.models.sort_by(|left, right| {
            left.provider
                .cmp(&right.provider)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.model.cmp(&right.model))
        });
        self.set_model_catalog(catalog)
    }

    pub fn quarantine_guru(
        &self,
        guru_id: &str,
        source: QuarantineSource,
        reason: impl Into<String>,
    ) {
        self.quarantined_gurus
            .write()
            .expect("Guru quarantine lock was poisoned")
            .entry(guru_id.to_owned())
            .or_default()
            .insert(source, reason.into());
    }

    pub fn clear_guru_quarantine(&self, guru_id: &str, source: QuarantineSource) {
        let mut quarantines = self
            .quarantined_gurus
            .write()
            .expect("Guru quarantine lock was poisoned");
        if let Some(sources) = quarantines.get_mut(guru_id) {
            sources.remove(&source);
            if sources.is_empty() {
                quarantines.remove(guru_id);
            }
        }
    }

    pub fn is_guru_quarantined(&self, guru_id: &str) -> bool {
        self.quarantined_gurus
            .read()
            .expect("Guru quarantine lock was poisoned")
            .contains_key(guru_id)
    }

    pub(crate) fn guru_access(&self, guru_id: &str) -> GuruAccess {
        let quarantines = self
            .quarantined_gurus
            .read()
            .expect("Guru quarantine lock was poisoned");
        let Some(sources) = quarantines.get(guru_id) else {
            return GuruAccess::Visible(GuruAvailability::Available);
        };
        if sources.contains_key(&QuarantineSource::Deletion) {
            return GuruAccess::Hidden;
        }
        if sources.contains_key(&QuarantineSource::MemoryWrite) {
            return GuruAccess::Visible(GuruAvailability::RecoveryRequired {
                reason: GuruRecoveryReason::InterruptedMemoryUpdate,
                action: GuruRecoveryAction::RecoverMemory,
            });
        }
        GuruAccess::Hidden
    }

    pub fn ensure_guru_available(&self, guru_id: &str) -> Result<(), CommandError> {
        match self.guru_access(guru_id) {
            GuruAccess::Visible(GuruAvailability::Available) => {}
            GuruAccess::Visible(GuruAvailability::RecoveryRequired { .. }) => {
                return Err(CommandError::new(
                    "guru_recovery_required",
                    "Guru requires recovery before it can be used",
                ));
            }
            GuruAccess::Hidden => {
                return Err(CommandError::new(
                    "guru_storage_unavailable",
                    "Guru storage is unavailable",
                ));
            }
        }
        #[cfg(not(test))]
        if let Some(profile) = self
            .store
            .get_guru(guru_id)
            .map_err(|error| CommandError::internal(error.to_string()))?
        {
            let expected = self
                .artifacts
                .app_data_dir
                .join("gurus")
                .join(guru_id)
                .join("workspace");
            if Path::new(&profile.memory_root) != expected {
                return Err(CommandError::new(
                    "guru_storage_unavailable",
                    "Guru storage is not owned by this Guru Terminal installation",
                ));
            }
        }
        Ok(())
    }

    pub fn register_run(
        &self,
        run_id: String,
        guru_id: String,
        kind: RunKind,
        target: RunTarget,
    ) -> Result<RunRegistration, CommandError> {
        self.ensure_guru_available(&guru_id)?;
        self.run_coordinator.register(
            RunSpec {
                run_id,
                guru_id: guru_id.clone(),
                kind,
                target,
            },
            || self.ensure_guru_available(&guru_id),
        )
    }

    pub async fn register_memory_write(
        &self,
        run_id: String,
        guru_id: String,
        target: RunTarget,
    ) -> Result<RunRegistration, CommandError> {
        self.reserve_memory_write(run_id, guru_id, target)?
            .wait()
            .await
    }

    pub fn reserve_memory_write(
        &self,
        run_id: String,
        guru_id: String,
        target: RunTarget,
    ) -> Result<PendingMemoryWrite, CommandError> {
        self.ensure_guru_available(&guru_id)?;
        self.run_coordinator.reserve_memory_write(
            RunSpec {
                run_id,
                guru_id: guru_id.clone(),
                kind: RunKind::MemoryWrite,
                target,
            },
            || self.ensure_guru_available(&guru_id),
        )
    }

    pub fn register_guru_recovery(
        &self,
        run_id: String,
        guru_id: String,
        action: GuruRecoveryAction,
    ) -> Result<RunRegistration, CommandError> {
        self.run_coordinator.register(
            RunSpec {
                run_id,
                guru_id: guru_id.clone(),
                kind: RunKind::MemoryWrite,
                target: RunTarget::MemoryWriteSession("pending-recovery".into()),
            },
            || match self.guru_access(&guru_id) {
                GuruAccess::Visible(GuruAvailability::RecoveryRequired {
                    action: required_action,
                    ..
                }) if action == required_action => Ok(()),
                GuruAccess::Visible(GuruAvailability::RecoveryRequired { .. }) => Err(
                    CommandError::invalid("requested Guru recovery action is not available"),
                ),
                GuruAccess::Visible(GuruAvailability::Available) => Err(CommandError::conflict(
                    "Guru does not currently require recovery",
                )),
                GuruAccess::Hidden => Err(CommandError::new(
                    "guru_storage_unavailable",
                    "Guru storage is unavailable",
                )),
            },
        )
    }

    pub fn register_chat_run(
        &self,
        run_id: String,
        guru_id: String,
        thread_id: String,
        control: ChatControlHandle,
    ) -> Result<RunRegistration, CommandError> {
        self.ensure_guru_available(&guru_id)?;
        self.run_coordinator.register_chat(
            RunSpec {
                run_id,
                guru_id: guru_id.clone(),
                kind: RunKind::Chat,
                target: RunTarget::ChatThread(thread_id),
            },
            control,
            || self.ensure_guru_available(&guru_id),
        )
    }

    pub async fn cancel_run(
        &self,
        run_id: &str,
        expected_kind: RunKind,
    ) -> Result<(), CommandError> {
        self.run_coordinator.cancel(run_id, expected_kind).await
    }

    pub async fn submit_chat_control(
        &self,
        guru_id: &str,
        thread_id: &str,
        kind: ChatControlKind,
        prompt: String,
    ) -> Result<AcceptedChatControl, CommandError> {
        self.ensure_guru_available(guru_id)?;
        self.run_coordinator
            .submit_chat_control(guru_id, thread_id, kind, prompt)
            .await
    }

    pub fn claim_run_completion(
        &self,
        run_id: &str,
        expected_kind: RunKind,
    ) -> Result<bool, CommandError> {
        self.run_coordinator.claim_completion(run_id, expected_kind)
    }

    pub fn begin_guru_deletion(&self, guru_id: &str) -> Result<(), CommandError> {
        self.run_coordinator.begin_guru_mutation(guru_id, || {
            self.quarantine_guru(
                guru_id,
                QuarantineSource::Deletion,
                "Guru deletion is in progress",
            )
        })
    }

    pub async fn forget_guru(&self, guru_id: &str) {
        self.clear_guru_quarantine(guru_id, QuarantineSource::Deletion);
    }

    #[cfg(test)]
    pub(crate) fn for_test(app_data_dir: PathBuf) -> Self {
        Self::for_test_with_store(app_data_dir, false, None)
    }

    #[cfg(test)]
    pub(crate) fn for_persistent_test(app_data_dir: PathBuf) -> Self {
        Self::for_test_with_store(app_data_dir, true, None)
    }

    #[cfg(test)]
    pub(crate) fn for_persistent_test_stage(app_data_dir: PathBuf, stage: &'static str) -> Self {
        Self::for_test_with_store(app_data_dir, true, Some(stage))
    }

    #[cfg(test)]
    pub(crate) fn close_for_restart_test(self) {
        // Restart tests intentionally reacquire the same app-data lock in one
        // process. Transfer the final lock reference out of AppState, prove no
        // hidden state clone can still own it, then establish an explicit
        // unlock boundary before constructing the replacement instance.
        let instance_lock = Arc::clone(&self._instance_lock);
        drop(self);
        assert_eq!(
            Arc::strong_count(&instance_lock),
            1,
            "restart fixture retained an AppState instance-lock owner"
        );
        FileExt::unlock(&instance_lock._file).unwrap();
    }

    #[cfg(test)]
    fn for_test_with_store(
        app_data_dir: PathBuf,
        persistent: bool,
        acquisition_stage: Option<&'static str>,
    ) -> Self {
        let app_data_dir = pin_app_data_dir(&app_data_dir).unwrap();
        for child in ["brokers", "connectors", "gurus", "mcp-pool", "pi", "runs"] {
            ensure_private_directory(&app_data_dir.join(child)).unwrap();
        }
        let mcp_pool = crate::mcp_pool::McpProcessPool::prepare(app_data_dir.join("mcp-pool"))
            .expect("test MCP pool");
        let instance_lock = Arc::new(AppInstanceLock::acquire(&app_data_dir).unwrap_or_else(
            |error| {
                panic!(
                    "{} could not acquire the test app-instance lock: {error:?}",
                    acquisition_stage.unwrap_or("test AppState initialization")
                )
            },
        ));
        let process_lease_dir = app_data_dir.join("process-leases");
        ensure_private_directory(&process_lease_dir).unwrap();
        prepare_lease_directory(&process_lease_dir).unwrap();
        if persistent {
            recover_orphaned_processes(&process_lease_dir).unwrap();
        }
        let finance_data = Arc::new(FinanceDataService::new().unwrap());
        let deletion_root =
            Arc::new(SecureDeletionRoot::open(&app_data_dir.canonicalize().unwrap()).unwrap());
        crate::run_scratch::sweep_stale_runs(&deletion_root).unwrap();
        let store = if persistent {
            SqliteStore::open(app_data_dir.join("guruterminal.sqlite3")).unwrap()
        } else {
            SqliteStore::open_in_memory().unwrap()
        };
        let deletion_recovery = deletion::recover(&store, &deletion_root).unwrap();
        let memory_finalization_recovery =
            crate::commands::memory_write::interrupted_memory_finalization_quarantines(&store)
                .unwrap();
        let model_catalog = store.get_model_catalog().unwrap().unwrap_or(ModelCatalog {
            models: vec![ConfiguredModel {
                id: "fixture".into(),
                name: "Fixture".into(),
                provider: "openai".into(),
                model: "fixture-model".into(),
                input: vec!["text".into()],
                reasoning: true,
                context_window: 128_000,
                max_tokens: 32_000,
                thinking_levels: vec!["off".into(), "low".into(), "medium".into(), "high".into()],
                thinking_level_map: std::collections::BTreeMap::new(),
                run_controls: vec![],
            }],
        });
        let model_visibility = store.get_model_visibility().unwrap().unwrap_or_default();
        let store = Arc::new(store);
        let maintenance = MaintenanceCoordinator::default();
        let updates = UpdateCoordinator::new(store.clone()).unwrap();
        let mut quarantined_gurus = deletion_recovery
            .quarantined_gurus
            .into_iter()
            .map(|(guru_id, reason)| {
                (
                    guru_id,
                    HashMap::from([(QuarantineSource::Deletion, reason)]),
                )
            })
            .collect::<HashMap<_, _>>();
        for (guru_id, reason) in memory_finalization_recovery {
            quarantined_gurus
                .entry(guru_id)
                .or_default()
                .insert(QuarantineSource::MemoryWrite, reason);
        }
        Self {
            store,
            runtime: None,
            artifacts: Arc::new(AppArtifacts {
                broker_dir: app_data_dir.join("brokers"),
                connector_config_dir: app_data_dir.join("connectors"),
                process_lease_dir,
                deletion_root,
                app_data_dir: app_data_dir.clone(),
                pi: None,
                finance_executable: None,
                compute: None,
                mcp_runtimes: BTreeMap::new(),
            }),
            mcp_pool,
            finance_data,
            run_coordinator: RunCoordinator::new(maintenance.clone()),
            provider_support: ProviderSupportCoordinator::new(maintenance.clone()),
            provider_model_cache: ProviderModelDiscoveryCache::default(),
            maintenance,
            updates,
            browser: BrowserManager::new(app_data_dir.join("browser-profile")),
            quarantined_gurus: Arc::new(RwLock::new(quarantined_gurus)),
            _instance_lock: instance_lock,
            model_catalog: Arc::new(RwLock::new(model_catalog)),
            model_visibility: Arc::new(RwLock::new(model_visibility)),
            fresh_install: false,
        }
    }
}

#[cfg(any(test, feature = "e2e"))]
pub(crate) fn with_live_pi_agent_data_dir_override(
    mut config: PiLaunchConfig,
) -> Result<PiLaunchConfig, CommandError> {
    let Some(path) = live_pi_agent_data_dir_override()? else {
        return Ok(config);
    };
    config.agent_data_dir = path;
    Ok(config)
}

#[cfg(any(test, feature = "e2e"))]
fn live_pi_agent_data_dir_override() -> Result<Option<PathBuf>, CommandError> {
    let Some(value) = std::env::var_os(LIVE_PI_AGENT_DATA_DIR_ENV) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(CommandError::new(
            "pi_unavailable",
            "live Pi agent data directory is invalid",
        ));
    }
    let path = PathBuf::from(value);
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| {
        CommandError::new(
            "pi_unavailable",
            "live Pi agent data directory is unavailable",
        )
    })?;
    if !path.is_absolute() || metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CommandError::new(
            "pi_unavailable",
            "live Pi agent data directory is invalid",
        ));
    }
    // Test code validates the directory entry only; it never enumerates, reads,
    // copies, serializes, or prints credentials. The real Pi process may still
    // refresh OAuth state in this connected directory during a successful run.
    Ok(Some(path))
}

fn resolve_pi_artifacts(
    executable: Option<PathBuf>,
    runtime_dir: Option<PathBuf>,
    agent_resource: Option<PathBuf>,
) -> Option<PiArtifacts> {
    let executable = executable?;
    let runtime_dir = runtime_dir?;
    let agent_resource = agent_resource?;
    let extension = agent_resource.join("guruterminal-extension.mjs");
    let provider_extension = agent_resource.join("guruterminal-provider-extension.mjs");
    let system_prompt = agent_resource.join("SYSTEM.md");
    if crate::agent_harness::validate_extension_bundle(&extension).is_err()
        || crate::agent_harness::validate_provider_extension_bundle(&provider_extension).is_err()
        || !system_prompt.is_file()
    {
        return None;
    }
    let bundled_skills =
        crate::agent_harness::run_skill_ids(&crate::agent_harness::default_skill_ids()).ok()?;
    crate::agent_harness::resolve_skill_paths(&agent_resource, &bundled_skills).ok()?;
    Some(PiArtifacts {
        executable,
        runtime_dir,
        extension,
        provider_extension,
        system_prompt,
        provider: String::new(),
        model: String::new(),
        thinking_level: String::new(),
        run_options: std::collections::BTreeMap::new(),
        provider_credential: None,
    })
}

fn resolve_file(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|path| path.is_file())
}

fn resolve_directory(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|path| path.is_dir())
}

fn may_replace_obsolete_local_database() -> bool {
    // Installed/release profiles still reject historical schemas without
    // mutation. Debug and E2E profiles are disposable local state, so an
    // obsolete database is discarded and replaced instead of crashing setup.
    cfg!(debug_assertions) || cfg!(feature = "e2e")
}

fn open_app_store(database_path: &Path) -> Result<(SqliteStore, bool), CommandError> {
    let existed = database_path.exists();
    if may_replace_obsolete_local_database() {
        SqliteStore::open_or_replace_obsolete(database_path)
    } else {
        SqliteStore::open(database_path).map(|store| (store, !existed))
    }
    .map_err(|error| CommandError::internal(error.to_string()))
}

fn pin_app_data_dir(path: &Path) -> Result<PathBuf, CommandError> {
    ensure_private_directory(path).map_err(|error| CommandError::internal(error.to_string()))?;
    // macOS exposes `/tmp` as a lexical alias of `/private/tmp`. Pin the
    // app-data root once and derive every persisted path from that exact
    // identity so profiles created through an alias remain app-owned.
    path.canonicalize()
        .map_err(|error| CommandError::internal(format!("app data path cannot be pinned: {error}")))
}

#[cfg(any(not(feature = "e2e"), test))]
fn app_data_dir_for_build(path: PathBuf, identifier: &str) -> PathBuf {
    if cfg!(debug_assertions) && identifier == PRODUCTION_APP_IDENTIFIER {
        path.join("development")
    } else {
        path
    }
}

#[cfg(feature = "e2e")]
fn e2e_app_data_dir(path: Option<PathBuf>) -> Result<PathBuf, CommandError> {
    let path = path.ok_or_else(|| {
        CommandError::internal("the E2E launcher must provide GURUTERMINAL_E2E_APP_DATA_DIR")
    })?;
    if !path.is_absolute() {
        return Err(CommandError::internal(
            "GURUTERMINAL_E2E_APP_DATA_DIR must be absolute",
        ));
    }
    Ok(path)
}

fn transient_broker_directory() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        // macOS commonly resolves `env::temp_dir()` below a long
        // `/var/folders/...` path, while Unix-domain sockets allow only 103
        // path bytes. Use the stable short alias and isolate the private
        // broker directory by effective user ID.
        PathBuf::from("/tmp").join(format!("guruterminal-{}", unsafe { libc::geteuid() }))
    }
    #[cfg(not(target_os = "macos"))]
    {
        env::temp_dir().join("guruterminal")
    }
}

fn target_binary(name: &str) -> String {
    let target = match (env::consts::ARCH, env::consts::OS) {
        ("aarch64", "macos") => "aarch64-apple-darwin",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("x86_64", "windows") => "x86_64-pc-windows-msvc",
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
        _ => return name.to_owned(),
    };
    let extension = if env::consts::OS == "windows" {
        ".exe"
    } else {
        ""
    };
    format!("{name}-{target}{extension}")
}

fn platform_binary(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

#[cfg(debug_assertions)]
fn local_debug_core_binary(manifest_dir: &Path) -> PathBuf {
    manifest_dir
        .join("../../../target/debug")
        .join(platform_binary("guruterminal-core"))
}

#[cfg(debug_assertions)]
fn local_debug_finance_candidates(manifest_dir: &Path) -> [PathBuf; 2] {
    [
        manifest_dir
            .join("resources/pi-runtime/finance-worker")
            .join(platform_binary("guruterminal-finance")),
        manifest_dir
            .join("../python/dist/guruterminal-finance")
            .join(platform_binary("guruterminal-finance")),
    ]
}

#[cfg(test)]
mod tests;
