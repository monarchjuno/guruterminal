use guruterminal_core::CanonicalMemoryKind;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const WORKSPACE_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Serialize)]
pub struct WorkspaceInit {
    pub root: String,
    pub created: Vec<String>,
    pub directories: Vec<String>,
    pub metadata_file: String,
    pub schema_version: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceMetadata {
    schema_version: u8,
}

pub fn initialize_workspace(root: &Path) -> Result<WorkspaceInit, String> {
    let root = absolute_path(root)?;
    if path_entry_exists(&root.join("guruterminal"))
        || path_entry_exists(&root.join(".guruterminal"))
    {
        return Err("the selected folder already contains Guru Terminal state".into());
    }
    let data_root = root.join("guruterminal");
    let mut created = Vec::new();
    let mut directories = Vec::new();

    for kind in CanonicalMemoryKind::ALL {
        let name = kind.slug();
        let path = data_root.join(name);
        fs::create_dir_all(&path).map_err(io_error)?;
        created.push(relative_display(&root, &path));
        directories.push(path.display().to_string());
    }

    let internal = root.join(".guruterminal");
    fs::create_dir_all(&internal).map_err(io_error)?;
    let metadata = internal.join("workspace.json");
    fs::write(
        &metadata,
        format!("{{\n  \"schema_version\": {WORKSPACE_SCHEMA_VERSION}\n}}\n"),
    )
    .map_err(io_error)?;
    created.push(relative_display(&root, &metadata));

    Ok(WorkspaceInit {
        root: root.display().to_string(),
        created,
        directories,
        metadata_file: metadata.display().to_string(),
        schema_version: WORKSPACE_SCHEMA_VERSION,
    })
}

pub fn require_workspace(root: &Path) -> Result<(), String> {
    match workspace_schema(root) {
        Ok(WORKSPACE_SCHEMA_VERSION) => Ok(()),
        Ok(version) => Err(format!(
            "unsupported Guru Terminal workspace schema {version}; expected {WORKSPACE_SCHEMA_VERSION}"
        )),
        Err(error) => Err(error),
    }
}

fn workspace_schema(root: &Path) -> Result<u8, String> {
    let data_root = root.join("guruterminal");
    if !data_root.is_dir()
        || CanonicalMemoryKind::ALL
            .iter()
            .any(|kind| !data_root.join(kind.slug()).is_dir())
        || path_entry_exists(&data_root.join("method"))
    {
        return Err("workspace is not initialized for Guru Terminal".into());
    }
    let metadata_path = root.join(".guruterminal/workspace.json");
    let bytes = fs::read(&metadata_path)
        .map_err(|_| "workspace is not initialized for Guru Terminal".to_string())?;
    let metadata: WorkspaceMetadata = serde_json::from_slice(&bytes)
        .map_err(|_| "Guru Terminal workspace metadata is malformed".to_string())?;
    Ok(metadata.schema_version)
}

fn path_entry_exists(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

pub fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map_err(io_error)
            .map(|current| current.join(path))
    }
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn io_error(error: std::io::Error) -> String {
    error.to_string()
}
