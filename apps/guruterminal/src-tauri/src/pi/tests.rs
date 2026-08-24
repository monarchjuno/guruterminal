use super::*;
use tokio::io::{duplex, AsyncWriteExt};

#[test]
fn launch_is_offline_and_exposes_only_explicit_extension_tools() {
    let config = PiLaunchConfig {
        executable: "pi".into(),
        runtime_dir: "runtime".into(),
        extension: "extension.mjs".into(),
        system_prompt: "SYSTEM.md".into(),
        agent_data_dir: "agent-data".into(),
        working_dir: "workbench".into(),
        private_run_dir: "run".into(),
        lease_dir: "leases".into(),
        broker_socket: "broker.sock".into(),
        broker_token: "secret".into(),
        provider: "openai".into(),
        model: "openai/gpt-5".into(),
        thinking_level: "medium".into(),
        run_options: std::collections::BTreeMap::new(),
        provider_credential: None,
        host_context: Some("{\"agent_runtime\":{\"mode\":\"chat\"}}".into()),
        skill_files: Vec::new(),
        session: None,
    };
    let args = config.rpc_arguments();
    for required in [
        "--mode",
        "rpc",
        "--no-session",
        "--no-builtin-tools",
        "--no-extensions",
        "--extension",
        "--no-skills",
        "--no-context-files",
        "--offline",
        "--thinking",
    ] {
        assert!(args.iter().any(|arg| arg == required));
    }
    assert!(!args.iter().any(|arg| arg == "--api-key"));
    let system_prompt_index = args
        .iter()
        .position(|arg| arg == "--system-prompt")
        .expect("system prompt flag");
    assert_eq!(args[system_prompt_index + 1], GURUTERMINAL_SYSTEM_PROMPT);
    assert_ne!(args[system_prompt_index + 1], "SYSTEM.md");
    let thinking_index = args
        .iter()
        .position(|arg| arg == "--thinking")
        .expect("thinking flag");
    assert_eq!(args[thinking_index + 1], "medium");
    assert!(!args.iter().any(|arg| arg == "--append-system-prompt"));
    assert!(!args
        .iter()
        .any(|arg| Some(arg) == config.host_context.as_ref()));
}

#[test]
fn persistent_session_uses_exact_directory_and_id_without_mutating_its_name() {
    let temporary = tempfile::tempdir().unwrap();
    let mut config = PiLaunchConfig {
        executable: "pi".into(),
        runtime_dir: "runtime".into(),
        extension: "extension.mjs".into(),
        system_prompt: "SYSTEM.md".into(),
        agent_data_dir: "agent-data".into(),
        working_dir: "workbench".into(),
        private_run_dir: "run".into(),
        lease_dir: "leases".into(),
        broker_socket: "broker.sock".into(),
        broker_token: "secret".into(),
        provider: "openai".into(),
        model: "openai/gpt-5".into(),
        thinking_level: "medium".into(),
        run_options: std::collections::BTreeMap::new(),
        provider_credential: None,
        host_context: Some("{}".into()),
        skill_files: Vec::new(),
        session: None,
    };
    let session_id = "123e4567-e89b-42d3-a456-426614174000";
    config.session = Some(PiSessionConfig {
        id: session_id.into(),
        directory: temporary.path().to_path_buf(),
    });
    let args = config.rpc_arguments();
    assert!(!args.iter().any(|arg| arg == "--no-session"));
    assert!(!args.iter().any(|arg| arg == "--name"));
    let directory = args.iter().position(|arg| arg == "--session-dir").unwrap();
    assert_eq!(args[directory + 1], temporary.path().to_string_lossy());
    let id = args.iter().position(|arg| arg == "--session-id").unwrap();
    assert_eq!(args[id + 1], session_id);
}

#[test]
fn launch_disables_discovery_but_passes_only_exact_bundled_skills() {
    let agent_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../agent");
    let skill_files = agent_harness::resolve_skill_paths(
        &agent_root,
        &agent_harness::run_skill_ids(&[agent_harness::RESEARCH_SKILL_ID.to_owned()]).unwrap(),
    )
    .unwrap();
    let config = PiLaunchConfig {
        executable: "pi".into(),
        runtime_dir: "runtime".into(),
        extension: agent_root.join("guruterminal-extension.mjs"),
        system_prompt: agent_root.join("SYSTEM.md"),
        agent_data_dir: "agent-data".into(),
        working_dir: "workbench".into(),
        private_run_dir: "run".into(),
        lease_dir: "leases".into(),
        broker_socket: "broker.sock".into(),
        broker_token: "secret".into(),
        provider: "openai".into(),
        model: "openai/gpt-5".into(),
        thinking_level: "medium".into(),
        run_options: std::collections::BTreeMap::new(),
        provider_credential: None,
        host_context: Some("{\"agent_runtime\":{\"mode\":\"chat\"}}".into()),
        skill_files: Vec::new(),
        session: None,
    }
    .with_skills(skill_files.clone())
    .unwrap();
    let args = config.rpc_arguments();
    assert!(args.iter().any(|arg| arg == "--no-skills"));
    let skill_flags: Vec<&String> = args
        .iter()
        .enumerate()
        .filter_map(|(index, arg)| (arg == "--skill").then(|| args.get(index + 1)).flatten())
        .collect();
    assert_eq!(
        skill_flags
            .iter()
            .map(|path| path.as_str())
            .collect::<Vec<_>>(),
        skill_files
            .iter()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>()
    );
    assert!(skill_files
        .iter()
        .any(|path| { path.ends_with("skills/research/SKILL.md") }));
    assert!(skill_files
        .iter()
        .any(|path| { path.ends_with("skills/valuation/SKILL.md") }));
    let environment = config.environment(Path::new("host-context.json"));
    let allowed: Vec<String> =
        serde_json::from_str(environment.get("GURUTERMINAL_SKILL_FILES").unwrap()).unwrap();
    assert_eq!(
        allowed,
        skill_files
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    );

    let temporary = tempfile::tempdir().unwrap();
    let copied = temporary.path().join("SKILL.md");
    std::fs::write(
        &copied,
        include_bytes!("../../../agent/skills/research/SKILL.md"),
    )
    .unwrap();
    assert!(config.clone().with_skills(vec![copied]).is_err());
}

#[test]
fn host_context_is_bounded_and_written_to_a_private_extension_file() {
    let temporary = tempfile::tempdir().unwrap();
    let private_run_dir = temporary.path().join("run");
    std::fs::create_dir(&private_run_dir).unwrap();
    let config = PiLaunchConfig {
        executable: "pi".into(),
        runtime_dir: "runtime".into(),
        extension: "extension.mjs".into(),
        system_prompt: "SYSTEM.md".into(),
        agent_data_dir: "agent-data".into(),
        working_dir: temporary.path().join("workbench"),
        private_run_dir,
        lease_dir: "leases".into(),
        broker_socket: "broker.sock".into(),
        broker_token: "secret".into(),
        provider: "openai".into(),
        model: "openai/gpt-5".into(),
        thinking_level: "medium".into(),
        run_options: std::collections::BTreeMap::new(),
        provider_credential: None,
        host_context: None,
        skill_files: Vec::new(),
        session: None,
    }
    .with_host_context("{\"agent_runtime\":{\"mode\":\"chat\"}}".into())
    .unwrap();
    let args = config.rpc_arguments();
    assert!(!args
        .iter()
        .any(|arg| Some(arg) == config.host_context.as_ref()));
    let context_file = write_host_context_file(&config).unwrap();
    assert_eq!(
        std::fs::read_to_string(context_file.path()).unwrap(),
        "{\"agent_runtime\":{\"mode\":\"chat\"}}"
    );
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(context_file.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        config
            .environment(context_file.path())
            .get("GURUTERMINAL_HOST_CONTEXT_FILE")
            .map(String::as_str),
        context_file.path().to_str()
    );
    assert!(config
        .clone()
        .with_host_context("x".repeat(MAX_HOST_CONTEXT_BYTES + 1))
        .is_err());
}

#[tokio::test]
async fn rpc_reader_uses_lf_and_strips_only_an_optional_cr() {
    let (mut writer, reader) = duplex(128);
    writer
        .write_all(b"{\"type\":\"one\"}\r\nrest\n")
        .await
        .unwrap();
    drop(writer);
    let mut reader = BufReader::new(reader);
    assert_eq!(
        read_rpc_frame(&mut reader).await.unwrap().unwrap(),
        br#"{"type":"one"}"#
    );
    assert_eq!(read_rpc_frame(&mut reader).await.unwrap().unwrap(), b"rest");
    assert!(read_rpc_frame(&mut reader).await.unwrap().is_none());
}

#[test]
fn child_environment_is_an_allowlist() {
    let config = PiLaunchConfig {
        executable: "pi".into(),
        runtime_dir: "runtime".into(),
        extension: "extension.mjs".into(),
        system_prompt: "SYSTEM.md".into(),
        agent_data_dir: "agent-data".into(),
        working_dir: "workbench".into(),
        private_run_dir: "run".into(),
        lease_dir: "leases".into(),
        broker_socket: "broker.sock".into(),
        broker_token: "secret".into(),
        provider: "openai".into(),
        model: "openai/gpt-5".into(),
        thinking_level: "medium".into(),
        run_options: std::collections::BTreeMap::new(),
        provider_credential: Some(("OPENAI_API_KEY".into(), "credential".into())),
        host_context: None,
        skill_files: Vec::new(),
        session: None,
    };
    let env = config.environment(Path::new("host-context.json"));
    assert_eq!(env.get("PI_OFFLINE").map(String::as_str), Some("1"));
    assert_eq!(
        env.get("OPENAI_API_KEY").map(String::as_str),
        Some("credential")
    );
    assert!(!env.contains_key("HOME"));
    assert!(!env.contains_key("PATH"));
    assert_eq!(
        env.get("GURUTERMINAL_HOST_CONTEXT_FILE")
            .map(String::as_str),
        Some("host-context.json")
    );
}

#[test]
fn only_agent_settled_finishes_abort_grace() {
    assert!(is_agent_settled(&json!({ "type": "agent_settled" })));
    assert!(!is_agent_settled(&json!({ "type": "agent_end" })));
    assert!(!is_agent_settled(&json!({ "type": "message_end" })));
}

#[test]
fn steering_uses_the_pinned_rpc_command_and_rejects_empty_messages() {
    assert_eq!(
        text_request("steer", "Challenge the cutoff assumption.").unwrap(),
        json!({
            "type": "steer",
            "message": "Challenge the cutoff assumption."
        })
    );
    assert!(matches!(
        text_request("steer", " \n "),
        Err(PiError::InvalidLaunchValue)
    ));
    assert!(matches!(
        text_request("follow_up", "obsolete command"),
        Err(PiError::InvalidLaunchValue)
    ));
    assert!(matches!(
        text_request("queue_message", "obsolete command"),
        Err(PiError::InvalidLaunchValue)
    ));
}

#[test]
fn image_prompts_use_pi_rpc_image_content() {
    assert_eq!(
        prompt_request(
            "Inspect this chart.",
            &[PiImageContent {
                data: "iVBORw==".into(),
                mime_type: "image/png".into(),
            }],
        )
        .unwrap(),
        json!({
            "type": "prompt",
            "message": "Inspect this chart.",
            "images": [{
                "type": "image",
                "data": "iVBORw==",
                "mimeType": "image/png"
            }]
        })
    );
    assert!(prompt_request(
        "Inspect this image.",
        &[PiImageContent {
            data: "payload".into(),
            mime_type: "image/svg+xml".into(),
        }],
    )
    .is_err());
}

#[tokio::test]
async fn rpc_write_times_out_when_the_child_stops_reading() {
    let (writer, _reader) = duplex(8);
    let writer = Mutex::new(writer);
    let bytes = vec![b'x'; 64 * 1024];
    assert!(matches!(
        write_rpc_bytes(&writer, &bytes, Duration::from_millis(10)).await,
        Err(PiError::WriteTimeout)
    ));
}

#[cfg(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "windows", target_arch = "x86_64")
))]
#[test]
fn launch_validation_rejects_unpinned_runtime_metadata() {
    let temporary = tempfile::tempdir().unwrap();
    let executable = temporary.path().join("pi");
    let runtime = temporary.path().join("runtime");
    let agent_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../agent");
    let extension = agent_root.join("guruterminal-extension.mjs");
    let prompt = agent_root.join("SYSTEM.md");
    std::fs::create_dir(&runtime).unwrap();
    std::fs::write(&executable, b"fixture").unwrap();
    std::fs::write(runtime.join(".pi-version"), PI_VERSION).unwrap();
    std::fs::write(
        runtime.join(".pi-archive.sha256"),
        pinned_pi_archive_sha256().unwrap(),
    )
    .unwrap();
    std::fs::write(
        runtime.join(".pi-executable.sha256"),
        file_sha256(&executable).unwrap(),
    )
    .unwrap();
    std::fs::write(
        runtime.join("package.json"),
        format!(r#"{{"version":"{PI_VERSION}"}}"#),
    )
    .unwrap();
    let config = PiLaunchConfig {
        executable,
        runtime_dir: runtime,
        extension,
        system_prompt: prompt,
        agent_data_dir: temporary.path().join("agent-data"),
        working_dir: temporary.path().join("workbench"),
        private_run_dir: temporary.path().join("run"),
        lease_dir: temporary.path().join("leases"),
        broker_socket: temporary.path().join("broker.sock"),
        broker_token: "secret".into(),
        provider: "openai".into(),
        model: "openai/gpt-5".into(),
        thinking_level: "medium".into(),
        run_options: std::collections::BTreeMap::new(),
        provider_credential: None,
        host_context: Some("{\"agent_runtime\":{\"mode\":\"chat\"}}".into()),
        skill_files: Vec::new(),
        session: None,
    };
    assert!(config.validate().is_ok());
    assert!(verify_pi_runtime(&config.executable, &config.runtime_dir).is_ok());
    let mut missing_context = config.clone();
    missing_context.host_context = None;
    assert!(matches!(
        missing_context.validate(),
        Err(PiError::InvalidLaunchValue)
    ));
    std::fs::write(&config.executable, b"substituted").unwrap();
    assert!(matches!(
        verify_pi_runtime(&config.executable, &config.runtime_dir),
        Err(PiError::UntrustedRuntime)
    ));
    std::fs::write(&config.executable, b"fixture").unwrap();
    std::fs::write(config.runtime_dir.join(".pi-version"), "0.0.0").unwrap();
    assert!(matches!(
        verify_pi_runtime(&config.executable, &config.runtime_dir),
        Err(PiError::UntrustedRuntime)
    ));
}
