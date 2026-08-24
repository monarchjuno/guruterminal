#![allow(dead_code)]

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_guruterminal-core")
}

pub fn temp_dir(suite: &str, label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "guruterminal-{suite}-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

pub fn command(arguments: &[&str]) -> Output {
    Command::new(bin()).args(arguments).output().unwrap()
}

pub fn init(root: &Path) {
    let output = command(&["init", root.to_str().unwrap(), "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub fn json_command(arguments: &[&str]) -> Value {
    let output = command(arguments);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[allow(clippy::too_many_arguments)]
pub fn write_record(
    root: &Path,
    kind: &str,
    id: &str,
    title: &str,
    summary: &str,
    extra_frontmatter: &str,
    body: &str,
    as_of: &str,
) {
    write_relative_record(
        root,
        &format!("{kind}/{}.md", id.replace(':', "-")),
        id,
        title,
        summary,
        extra_frontmatter,
        body,
        as_of,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn write_relative_record(
    root: &Path,
    relative: &str,
    id: &str,
    title: &str,
    summary: &str,
    extra_frontmatter: &str,
    body: &str,
    as_of: &str,
) {
    let path = root.join("guruterminal").join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        format!(
            "---\nid: {id}\ntitle: {title}\nsummary: {summary}\n\
             as_of: {as_of}\n\
             {extra_frontmatter}---\n\n{body}\n"
        ),
    )
    .unwrap();
}
