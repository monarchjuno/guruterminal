pub mod agent_harness;
pub mod agent_tools;
pub mod app;
pub mod artifact_trust;
pub mod broker;
pub mod browser;
pub mod chart_engine;
pub mod chat_artifacts;
pub mod chat_control;
pub mod chat_execution_session;
pub mod chat_progress;
mod chat_turn;
pub mod commands;
pub mod compute;
pub mod deletion;
pub mod document;
pub mod domain;
mod external_browser;
pub mod finance;
pub mod finance_credentials;
pub mod finance_data;
mod guru_root;
pub mod hashing;
mod json_pointer;
mod maintenance;
pub mod marketplace;
pub mod mcp;
mod mcp_pool;
mod memory_finalization;
mod memory_git;
pub mod pi;
pub mod pi_execution;
pub mod pi_response;
pub mod pinned_root;
pub mod process_lease;
pub mod provider_connection;
pub mod run_coordinator;
mod run_id;
mod run_scratch;
pub mod runtime;
pub mod secure_delete;
pub mod settings;
pub mod snapshot;
pub mod store;
pub mod support_coordinator;
pub mod updater;
pub mod user_skill;
pub mod web;
#[cfg(feature = "webdriver")]
mod webdriver_attach;
#[cfg(windows)]
pub(crate) mod windows_fs;
mod workbench;

#[cfg(all(feature = "webdriver", not(debug_assertions)))]
compile_error!("WebDriver is forbidden in release builds");

#[cfg(any(not(feature = "e2e"), test))]
pub(crate) const DEVELOPMENT_APP_IDENTIFIER: &str = "com.monarchjuno.guruterminal";
#[cfg(feature = "e2e")]
pub(crate) const E2E_APP_IDENTIFIER: &str = "com.monarchjuno.guruterminal.e2e";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();
    let builder = tauri::Builder::default();
    // `main` starts the Windows UI on an explicitly sized worker thread so
    // debug startup has enough stack. Opt Tauri's event loop into that
    // supported execution mode before constructing windows or plugins.
    #[cfg(windows)]
    let builder = builder.any_thread();
    #[cfg(any(target_os = "macos", windows))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        use tauri::Manager;

        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }));
    #[cfg(all(any(target_os = "macos", windows), not(debug_assertions)))]
    let builder = if updater::configured(&context.config().plugins, env!("CARGO_PKG_VERSION")) {
        builder.plugin(tauri_plugin_updater::Builder::new().build())
    } else {
        builder
    };
    #[cfg(feature = "webdriver")]
    let builder = {
        #[cfg(feature = "e2e")]
        assert_eq!(
            context.config().identifier,
            E2E_APP_IDENTIFIER,
            "the e2e WebDriver requires the isolated Guru Terminal E2E identifier"
        );
        #[cfg(not(feature = "e2e"))]
        assert_eq!(
            context.config().identifier,
            DEVELOPMENT_APP_IDENTIFIER,
            "debug WebDriver attaches only to the development app identifier"
        );
        webdriver_attach::wait_for_bind_port();
        builder.plugin(tauri_plugin_wdio_webdriver::init())
    };

    builder
        .setup(|app| {
            use tauri::Manager;

            let state = match app::AppState::initialize(app.handle()) {
                Ok(state) => state,
                Err(error) if app::is_app_instance_conflict(&error) => {
                    // The official single-instance plugin normally exits before
                    // setup after notifying the primary process. Its macOS Unix
                    // socket can nevertheless be absent during a simultaneous
                    // launch or after an older build, while the independent
                    // app-data lock still proves that a primary process is live.
                    // Treat only that exact lock conflict as a normal relaunch;
                    // returning the error from a macOS setup hook would panic in
                    // the native event-loop callback and show a crash report.
                    app.cleanup_before_exit();
                    std::process::exit(0);
                }
                Err(error) => {
                    eprintln!("Guru Terminal failed to start: {error}");
                    app.cleanup_before_exit();
                    std::process::exit(1);
                }
            };
            if let Err(error) = tauri::async_runtime::block_on(async {
                if state.fresh_install {
                    commands::bootstrap_default_guru(&state).await?;
                }
                Ok::<(), app::CommandError>(())
            }) {
                eprintln!("Guru Terminal failed to start: {error}");
                app.cleanup_before_exit();
                std::process::exit(1);
            }
            let update_coordinator = state.updates.clone();
            app.manage(state);
            updater::start_auto_update_loop(app.handle().clone(), update_coordinator);
            #[cfg(feature = "webdriver")]
            webdriver_attach::require_status_endpoint();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::guru::guru_list,
            commands::guru::guru_select,
            commands::guru::guru_recover,
            commands::guru::guru_create,
            commands::guru::guru_import_memory,
            commands::guru::guru_export_memory,
            commands::guru::guru_rename,
            commands::guru::guru_delete,
            commands::guru::agent_skill_catalog,
            commands::guru::agent_skills_update,
            commands::records::chat_create,
            commands::records::chat_rename,
            commands::records::chat_delete,
            commands::records::chat_attachment_read,
            commands::records::chat_artifact_list,
            commands::records::chat_artifact_read,
            commands::chat_runtime::chat_send,
            commands::chat_runtime::chat_steer,
            commands::chat_runtime::chat_abort,
            commands::records::library_search,
            commands::records::library_read,
            commands::memory_crud::library_memory_create,
            commands::memory_crud::library_memory_update,
            commands::memory_crud::library_memory_delete,
            commands::memory_crud::library_memory_revert,
            commands::model_catalog_get,
            commands::model_visibility_update,
            commands::run_activity_list,
            provider_connection::provider_models,
            provider_connection::provider_configure,
            provider_connection::provider_connect,
            provider_connection::provider_connect_cancel,
            provider_connection::provider_connect_open_browser,
            provider_connection::provider_disconnect,
            marketplace::marketplace_snapshot,
            marketplace::guru_capability_list,
            marketplace::guru_capability_enable,
            marketplace::connector_config::marketplace_connector_configure,
            marketplace::guru_capability_disable,
            marketplace::credentials::marketplace_credential_save,
            marketplace::credentials::marketplace_credential_verify,
            marketplace::credentials::marketplace_credential_delete,
            commands::open_external_url,
            browser::browser_tab_open,
            browser::browser_tab_navigate,
            browser::browser_tab_history,
            browser::browser_tab_reload,
            browser::browser_tab_set_bounds,
            browser::browser_tab_close,
            browser::browser_tabs_reset,
            updater::update_status,
            updater::update_check,
            updater::update_install,
        ])
        .run(context)
        .expect("failed to run Guru Terminal");
}
