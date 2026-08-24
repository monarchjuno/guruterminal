use super::*;
use std::{
    collections::{BTreeSet, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

use regex::Regex;

use crate::workbench::{
    WorkbenchStore, MAX_READ_LINES, MAX_TOOL_OUTPUT_BYTES, MAX_WORKBENCH_FILE_BYTES,
};

const MAX_WALK_ENTRIES: usize = 4_000;
const MAX_BINARY_WARNING_PATHS: usize = 8;
const MAX_WORKBENCH_QUERY_TEXT_BYTES: usize = 4_096;
const MAX_LS_RESULTS: u32 = 500;
const MAX_SEARCH_RESULTS: u32 = 200;
const MAX_GREP_CONTEXT: u32 = 3;

#[derive(Debug)]
struct WalkEntry {
    path: PathBuf,
    relative: String,
    is_directory: bool,
    is_file: bool,
}

#[derive(Default)]
struct BoundedOutput {
    text: String,
    truncated: bool,
}

impl BoundedOutput {
    fn push_line(&mut self, line: &str) {
        if self.truncated {
            return;
        }
        let separator = usize::from(!self.text.is_empty());
        let remaining = MAX_TOOL_OUTPUT_BYTES.saturating_sub(self.text.len());
        if remaining <= separator {
            self.truncated = true;
            return;
        }
        if separator == 1 {
            self.text.push('\n');
        }
        let remaining = MAX_TOOL_OUTPUT_BYTES.saturating_sub(self.text.len());
        if line.len() <= remaining {
            self.text.push_str(line);
            return;
        }
        let mut end = remaining;
        while end > 0 && !line.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&line[..end]);
        self.truncated = true;
    }

    fn finish(mut self, empty: &str, suffix: &str) -> String {
        if self.text.is_empty() && !self.truncated {
            self.text.push_str(empty);
        }
        if !self.truncated {
            self.text.push_str(suffix);
            if self.text.len() > MAX_TOOL_OUTPUT_BYTES {
                self.truncated = true;
            }
        }
        if self.truncated {
            let mut end = self.text.len().min(MAX_TOOL_OUTPUT_BYTES);
            while end > 0 && !self.text.is_char_boundary(end) {
                end -= 1;
            }
            self.text.truncate(end);
            self.text.push_str("\n\n[Output truncated at 50KB]");
        }
        self.text
    }
}

impl AppToolExecutor {
    fn workbench_root(&self) -> Result<PathBuf, BrokerError> {
        let root = super::super::guru::managed_guru_dir(&self.state, &self.guru_id)
            .map_err(|error| BrokerError::Execution(error.to_string()))?
            .join("workbench");
        let metadata = fs::symlink_metadata(&root)
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        if is_link_like(&metadata) || !metadata.is_dir() {
            return Err(BrokerError::Execution(
                "Workbench is not a directory".into(),
            ));
        }
        root.canonicalize()
            .map_err(|error| BrokerError::Execution(error.to_string()))
    }

    fn workbench_store(&self) -> Result<WorkbenchStore, BrokerError> {
        WorkbenchStore::open(self.guru_id.clone(), self.workbench_root()?)
    }

    pub(super) fn workbench_read(&self, params: Value) -> Result<Value, BrokerError> {
        let object = exact_object(&params, &["path"], &["offset", "limit"])?;
        let path = required_path(object)?;
        let offset = optional_line(object, "offset")?;
        let limit = optional_line(object, "limit")?;
        if let Some(offset) = offset {
            if offset < 1 {
                return Err(BrokerError::Malformed);
            }
        }
        if let Some(limit) = limit {
            if !(1..=MAX_READ_LINES).contains(&limit) {
                return Err(BrokerError::Malformed);
            }
        }
        self.workbench_store()?.read(path, offset, limit)
    }

    pub(super) fn workbench_write(&self, params: Value) -> Result<Value, BrokerError> {
        let object = exact_object(&params, &["path", "content"], &["expected_revision"])?;
        let path = required_path(object)?;
        let content = object
            .get("content")
            .and_then(Value::as_str)
            .ok_or(BrokerError::Malformed)?;
        if content.len() > MAX_WORKBENCH_FILE_BYTES {
            return Err(BrokerError::Execution("Workbench file is too large".into()));
        }
        let expected_revision = optional_revision(object)?;
        self.workbench_store()?
            .write(path, content, expected_revision)
    }

    pub(super) fn workbench_edit(&self, params: Value) -> Result<Value, BrokerError> {
        let object = exact_object(
            &params,
            &["path", "old_text", "new_text", "expected_revision"],
            &[],
        )?;
        let path = required_path(object)?;
        let old_text = object
            .get("old_text")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(BrokerError::Malformed)?;
        let new_text = object
            .get("new_text")
            .and_then(Value::as_str)
            .ok_or(BrokerError::Malformed)?;
        let expected_revision = object
            .get("expected_revision")
            .and_then(Value::as_str)
            .ok_or(BrokerError::Malformed)?;
        self.workbench_store()?
            .edit(path, old_text, new_text, expected_revision)
    }

    pub(super) fn workbench_list(&self, params: Value) -> Result<Value, BrokerError> {
        let object = exact_object(&params, &[], &["path", "limit"])?;
        let input_path = workbench_query_text(object, "path", ".", true)?;
        let limit = workbench_query_limit(object, MAX_LS_RESULTS)? as usize;
        let root = self.workbench_root()?;
        let path = resolve_existing_workbench_path(&root, input_path)?;
        if !fs::symlink_metadata(&path)
            .map_err(|error| BrokerError::Execution(error.to_string()))?
            .is_dir()
        {
            return Err(BrokerError::Execution(
                "Workbench path is not a directory".into(),
            ));
        }
        let (entries, walk_truncated) = directory_entries(&root, &path, limit + 1)?;
        let has_more = entries.len() > limit;
        let mut output = BoundedOutput::default();
        let mut count = 0_usize;
        for entry in entries.into_iter().take(limit) {
            let kind = if entry.is_directory { "dir " } else { "file" };
            let name = entry
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    BrokerError::Execution("Workbench path is not valid UTF-8".into())
                })?;
            output.push_line(&format!("{kind}\t{name}"));
            count += 1;
        }
        let output_truncated = output.truncated;
        Ok(json!({
            "text": output.finish("(empty)", ""),
            "count": count,
            "truncated": has_more || walk_truncated || output_truncated,
        }))
    }

    pub(super) fn workbench_find(&self, params: Value) -> Result<Value, BrokerError> {
        let object = exact_object(&params, &["pattern"], &["path", "limit"])?;
        let pattern = workbench_query_text(object, "pattern", "", false)?;
        let input_path = workbench_query_text(object, "path", ".", true)?;
        let limit = workbench_query_limit(object, MAX_SEARCH_RESULTS)? as usize;
        let matcher = glob_matcher(pattern)?;
        let root = self.workbench_root()?;
        let start = resolve_existing_workbench_path(&root, input_path)?;
        if !fs::symlink_metadata(&start)
            .map_err(|error| BrokerError::Execution(error.to_string()))?
            .is_dir()
        {
            return Err(BrokerError::Execution(
                "Workbench path is not a directory".into(),
            ));
        }
        let (entries, walk_truncated) = walk_workbench(&root, &start)?;
        let mut count = 0_usize;
        let mut has_more = false;
        let mut output = BoundedOutput::default();
        for entry in entries {
            if !matcher.is_match(&entry.relative) {
                continue;
            }
            if count < limit {
                output.push_line(&entry.relative);
                count += 1;
            } else {
                has_more = true;
            }
        }
        let output_truncated = output.truncated;
        Ok(json!({
            "text": output.finish("No files found", ""),
            "count": count,
            "truncated": has_more || walk_truncated || output_truncated,
        }))
    }

    pub(super) fn workbench_grep(&self, params: Value) -> Result<Value, BrokerError> {
        let object = exact_object(&params, &["pattern"], &["path", "glob", "limit", "context"])?;
        let pattern = workbench_query_text(object, "pattern", "", false)?;
        let input_path = workbench_query_text(object, "path", ".", true)?;
        let glob = match object.get("glob") {
            Some(_) => Some(glob_matcher(workbench_query_text(
                object, "glob", "", false,
            )?)?),
            None => None,
        };
        let limit = workbench_query_limit(object, MAX_SEARCH_RESULTS)? as usize;
        let context = optional_line(object, "context")?.unwrap_or(0);
        if context > MAX_GREP_CONTEXT {
            return Err(BrokerError::Malformed);
        }
        let expression = Regex::new(pattern).map_err(|_| {
            BrokerError::Execution("grep pattern is not a supported regular expression".into())
        })?;
        let root = self.workbench_root()?;
        let start = resolve_existing_workbench_path(&root, input_path)?;
        let metadata = fs::symlink_metadata(&start)
            .map_err(|error| BrokerError::Execution(error.to_string()))?;
        let (entries, walk_truncated) = if metadata.is_file() {
            (
                vec![WalkEntry {
                    relative: relative_workbench_path(&root, &start)?,
                    path: start,
                    is_directory: false,
                    is_file: true,
                }],
                false,
            )
        } else if metadata.is_dir() {
            walk_workbench(&root, &start)?
        } else {
            return Err(BrokerError::Execution(
                "Workbench path is not a bounded regular file or directory".into(),
            ));
        };

        let mut output = BoundedOutput::default();
        let mut match_count = 0_usize;
        let mut skipped_binary = Vec::new();
        for entry in entries {
            if match_count >= limit {
                break;
            }
            if !entry.is_file
                || glob
                    .as_ref()
                    .is_some_and(|matcher| !matcher.is_match(&entry.relative))
            {
                continue;
            }
            let Some(text) = read_search_file(&entry.path)? else {
                continue;
            };
            let text = match text {
                SearchFile::Text(text) => text,
                SearchFile::Binary => {
                    skipped_binary.push(entry.relative);
                    continue;
                }
            };
            let lines = text.split('\n').collect::<Vec<_>>();
            let matches = lines
                .iter()
                .map(|line| expression.is_match(line))
                .collect::<Vec<_>>();
            let mut emitted = BTreeSet::new();
            for (index, matched) in matches.iter().copied().enumerate() {
                if !matched || match_count >= limit {
                    continue;
                }
                let from = index.saturating_sub(context as usize);
                let to = (index + context as usize).min(lines.len().saturating_sub(1));
                for line_index in from..=to {
                    if !emitted.insert(line_index) {
                        continue;
                    }
                    let separator = if matches[line_index] { ':' } else { '-' };
                    output.push_line(&format!(
                        "{}{separator}{}{separator}{}",
                        entry.relative,
                        line_index + 1,
                        lines[line_index]
                    ));
                }
                match_count += 1;
            }
        }
        let warning = binary_warning(&skipped_binary);
        let warnings = warning
            .as_ref()
            .map(|warning| vec![warning.clone()])
            .unwrap_or_default();
        let warning_suffix = warning
            .as_deref()
            .map(|warning| format!("\n\n[{warning}]"))
            .unwrap_or_default();
        let limit_reached = match_count >= limit;
        let output_truncated = output.truncated
            || output.text.len().saturating_add(warning_suffix.len()) > MAX_TOOL_OUTPUT_BYTES;
        Ok(json!({
            "text": output.finish("No matches", &warning_suffix),
            "count": match_count,
            "skipped_binary": skipped_binary.len(),
            "skipped_binary_paths": skipped_binary
                .iter()
                .take(MAX_BINARY_WARNING_PATHS)
                .collect::<Vec<_>>(),
            "truncated": limit_reached || walk_truncated || output_truncated,
            "warnings": warnings,
        }))
    }
}

enum SearchFile {
    Text(String),
    Binary,
}

fn workbench_query_text<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
    default: &'a str,
    allow_empty: bool,
) -> Result<&'a str, BrokerError> {
    let value = match object.get(key) {
        None => default,
        Some(value) => value.as_str().ok_or(BrokerError::Malformed)?,
    };
    if value.contains('\0')
        || value.len() > MAX_WORKBENCH_QUERY_TEXT_BYTES
        || (!allow_empty && value.is_empty())
    {
        return Err(BrokerError::Malformed);
    }
    Ok(if allow_empty && value.is_empty() {
        default
    } else {
        value
    })
}

fn workbench_query_limit(
    object: &serde_json::Map<String, Value>,
    maximum: u32,
) -> Result<u32, BrokerError> {
    let limit = optional_line(object, "limit")?.unwrap_or(maximum);
    if !(1..=maximum).contains(&limit) {
        return Err(BrokerError::Malformed);
    }
    Ok(limit)
}

fn resolve_existing_workbench_path(root: &Path, input: &str) -> Result<PathBuf, BrokerError> {
    let requested = Path::new(input);
    if requested.is_absolute() {
        return Err(BrokerError::Execution(
            "Workbench path must be relative".into(),
        ));
    }
    let mut candidate = root.to_path_buf();
    for component in requested.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(BrokerError::Execution(
                    "Workbench path must be relative".into(),
                ));
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if candidate == root {
                    return Err(BrokerError::Execution(
                        "Path is outside this Guru's workbench".into(),
                    ));
                }
                candidate.pop();
            }
            Component::Normal(value) => candidate.push(value),
        }
    }
    if !candidate.starts_with(root) {
        return Err(BrokerError::Execution(
            "Path is outside this Guru's workbench".into(),
        ));
    }
    let mut current = root.to_path_buf();
    for component in candidate
        .strip_prefix(root)
        .map_err(|_| BrokerError::Execution("Workbench path is invalid".into()))?
        .components()
    {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                BrokerError::Execution("Workbench path does not exist".into())
            } else {
                BrokerError::Execution(error.to_string())
            }
        })?;
        if is_link_like(&metadata) {
            let target_outside = current
                .canonicalize()
                .ok()
                .is_some_and(|target| !target.starts_with(root));
            let message = if current != candidate || target_outside {
                "Path escapes this Guru's workbench through a symlink"
            } else {
                "Workbench tools do not follow symbolic links"
            };
            return Err(BrokerError::Execution(message.into()));
        }
    }
    Ok(candidate)
}

fn directory_entries(
    root: &Path,
    directory: &Path,
    maximum: usize,
) -> Result<(Vec<WalkEntry>, bool), BrokerError> {
    let directory_metadata = fs::symlink_metadata(directory)
        .map_err(|error| BrokerError::Execution(error.to_string()))?;
    if is_link_like(&directory_metadata) {
        return Err(BrokerError::Execution(
            "Path escapes this Guru's workbench through a symlink".into(),
        ));
    }
    if !directory_metadata.is_dir() {
        return Err(BrokerError::Execution(
            "Workbench path is not a directory".into(),
        ));
    }
    if !directory
        .canonicalize()
        .map_err(|error| BrokerError::Execution(error.to_string()))?
        .starts_with(root)
    {
        return Err(BrokerError::Execution(
            "Path escapes this Guru's workbench through a symlink".into(),
        ));
    }
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in
        fs::read_dir(directory).map_err(|error| BrokerError::Execution(error.to_string()))?
    {
        let entry = entry.map_err(|error| BrokerError::Execution(error.to_string()))?;
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(BrokerError::Execution(error.to_string())),
        };
        if is_link_like(&metadata) {
            continue;
        }
        let Some(relative) = relative_workbench_path(root, &path).ok() else {
            continue;
        };
        if entries.len() >= maximum {
            truncated = true;
            continue;
        }
        entries.push(WalkEntry {
            path,
            relative,
            is_directory: metadata.is_dir(),
            is_file: metadata.is_file(),
        });
    }
    entries.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok((entries, truncated))
}

fn walk_workbench(root: &Path, start: &Path) -> Result<(Vec<WalkEntry>, bool), BrokerError> {
    let mut queue = VecDeque::from([start.to_path_buf()]);
    let mut visited = Vec::new();
    let mut truncated = false;
    while let Some(directory) = queue.pop_front() {
        let remaining = MAX_WALK_ENTRIES.saturating_sub(visited.len());
        if remaining == 0 {
            truncated = true;
            break;
        }
        let (children, child_truncated) = directory_entries(root, &directory, remaining)?;
        truncated |= child_truncated;
        for child in children {
            if child.is_directory {
                queue.push_back(child.path.clone());
            }
            visited.push(child);
        }
    }
    if !queue.is_empty() {
        truncated = true;
    }
    Ok((visited, truncated))
}

fn relative_workbench_path(root: &Path, path: &Path) -> Result<String, BrokerError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| BrokerError::Execution("Workbench path is invalid".into()))?;
    let mut components = Vec::new();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(BrokerError::Execution("Workbench path is invalid".into()));
        };
        components.push(
            component.to_str().ok_or_else(|| {
                BrokerError::Execution("Workbench path is not valid UTF-8".into())
            })?,
        );
    }
    Ok(if components.is_empty() {
        ".".into()
    } else {
        components.join("/")
    })
}

fn glob_matcher(pattern: &str) -> Result<Regex, BrokerError> {
    let mut expression = String::with_capacity(pattern.len() + 2);
    expression.push('^');
    let mut characters = pattern.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '*' if characters.peek() == Some(&'*') => {
                characters.next();
                expression.push_str(".*");
            }
            '*' => expression.push_str("[^/]*"),
            '?' => expression.push_str("[^/]"),
            '.' | '+' | '^' | '$' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\' => {
                expression.push('\\');
                expression.push(character);
            }
            _ => expression.push(character),
        }
    }
    expression.push('$');
    Regex::new(&expression).map_err(|_| BrokerError::Malformed)
}

fn read_search_file(path: &Path) -> Result<Option<SearchFile>, BrokerError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(BrokerError::Execution(error.to_string())),
    };
    if is_link_like(&metadata)
        || !metadata.is_file()
        || metadata.len() > MAX_WORKBENCH_FILE_BYTES as u64
    {
        return Ok(None);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound
                    | io::ErrorKind::PermissionDenied
                    | io::ErrorKind::InvalidInput
            ) || is_symlink_open_error(&error) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(BrokerError::Execution(error.to_string())),
    };
    if !bounded_regular_file(&file)? {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take((MAX_WORKBENCH_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| BrokerError::Execution(error.to_string()))?;
    if bytes.len() > MAX_WORKBENCH_FILE_BYTES {
        return Ok(None);
    }
    if bytes.contains(&0) {
        return Ok(Some(SearchFile::Binary));
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Some(SearchFile::Text(text))),
        Err(_) => Ok(Some(SearchFile::Binary)),
    }
}

#[cfg(unix)]
fn is_symlink_open_error(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ELOOP)
}

#[cfg(not(unix))]
fn is_symlink_open_error(_error: &io::Error) -> bool {
    false
}

#[cfg(windows)]
fn is_link_like(metadata: &fs::Metadata) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn bounded_regular_file(file: &File) -> Result<bool, BrokerError> {
    file.metadata()
        .map(|metadata| metadata.is_file() && metadata.len() <= MAX_WORKBENCH_FILE_BYTES as u64)
        .map_err(|error| BrokerError::Execution(error.to_string()))
}

fn binary_warning(paths: &[String]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    let shown = paths
        .iter()
        .take(MAX_BINARY_WARNING_PATHS)
        .cloned()
        .collect::<Vec<_>>();
    let extra = paths.len().saturating_sub(shown.len());
    let noun = if paths.len() == 1 {
        "binary file"
    } else {
        "binary files"
    };
    let more = if extra > 0 {
        format!(", and {extra} more")
    } else {
        String::new()
    };
    Some(format!(
        "Skipped {} {noun}: {}{more}",
        paths.len(),
        shown.join(", ")
    ))
}

fn required_path(object: &serde_json::Map<String, Value>) -> Result<&str, BrokerError> {
    object
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(BrokerError::Malformed)
}

fn optional_line(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<u32>, BrokerError> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .map(Some)
            .ok_or(BrokerError::Malformed),
    }
}

fn optional_revision(object: &serde_json::Map<String, Value>) -> Result<Option<&str>, BrokerError> {
    match object.get("expected_revision") {
        None => Ok(None),
        Some(value) => value.as_str().map(Some).ok_or(BrokerError::Malformed),
    }
}
