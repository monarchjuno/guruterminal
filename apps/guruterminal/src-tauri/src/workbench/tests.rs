use super::*;
use crate::broker::BrokerError;
use serde_json::Value;
use std::{
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
};

#[cfg(unix)]
use std::io;

fn store(temporary: &tempfile::TempDir) -> (WorkbenchStore, PathBuf) {
    let root = temporary.path().join("workbench");
    fs::create_dir_all(&root).unwrap();
    let store = WorkbenchStore::open(format!("guru-{}", Uuid::new_v4()), root.clone()).unwrap();
    (store, root)
}

fn status(value: &Value) -> &str {
    value["status"].as_str().unwrap_or("ok")
}

fn execution(error: BrokerError) -> String {
    match error {
        BrokerError::Execution(message) => message,
        other => panic!("expected Execution, got {other:?}"),
    }
}

#[test]
fn workbench_create_and_read_return_canonical_path_and_byte_revision() {
    let temporary = tempfile::tempdir().unwrap();
    let (store, root) = store(&temporary);
    let created = store
        .write("notes/idea.md", "durable insight", None)
        .unwrap();
    assert_eq!(status(&created), "ok");
    assert_eq!(created["path"], "notes/idea.md");
    assert_eq!(created["bytes"], b"durable insight".len());
    let expected = revision_token("notes/idea.md", b"durable insight");
    assert_eq!(created["revision"], expected);
    assert_eq!(
        fs::read_to_string(root.join("notes/idea.md")).unwrap(),
        "durable insight"
    );
    let read = store.read("notes/idea.md", None, None).unwrap();
    assert_eq!(read["content"], "durable insight");
    assert_eq!(read["path"], "notes/idea.md");
    assert_eq!(read["total_lines"], 1);
    assert_eq!(read["revision"], expected);
}

#[test]
fn workbench_create_conflicts_when_the_path_already_exists() {
    let temporary = tempfile::tempdir().unwrap();
    let (store, root) = store(&temporary);
    store.write("note.md", "original", None).unwrap();
    let conflict = store.write("note.md", "stale overwrite", None).unwrap();
    assert_eq!(status(&conflict), "conflict");
    assert_eq!(conflict["revision"], revision_token("note.md", b"original"));
    assert_eq!(
        fs::read_to_string(root.join("note.md")).unwrap(),
        "original"
    );
}

#[test]
fn workbench_replace_requires_expected_revision_and_rejects_stale_tokens() {
    let temporary = tempfile::tempdir().unwrap();
    let (store, root) = store(&temporary);
    let created = store.write("note.md", "original", None).unwrap();
    let revision = created["revision"].as_str().unwrap().to_owned();
    assert_eq!(
        store.write("note.md", "replaced", None).unwrap()["status"],
        "conflict"
    );
    let conflict = store
        .write("note.md", "replaced", Some(&"ab".repeat(32)))
        .unwrap();
    assert_eq!(status(&conflict), "conflict");
    assert_eq!(conflict["revision"], revision);
    assert_eq!(
        fs::read_to_string(root.join("note.md")).unwrap(),
        "original"
    );
    let replaced = store.write("note.md", "replaced", Some(&revision)).unwrap();
    assert_eq!(status(&replaced), "ok");
    assert_eq!(replaced["revision"], revision_token("note.md", b"replaced"));
    assert_eq!(
        fs::read_to_string(root.join("note.md")).unwrap(),
        "replaced"
    );
}

#[test]
fn workbench_edit_requires_expected_revision_and_preserves_bytes_on_failed_edit() {
    let temporary = tempfile::tempdir().unwrap();
    let (store, root) = store(&temporary);
    let created = store.write("note.md", "alpha alpha", None).unwrap();
    let revision = created["revision"].as_str().unwrap().to_owned();
    assert!(matches!(
        store.edit("note.md", "alpha", "beta", "not-a-revision"),
        Err(BrokerError::Malformed)
    ));
    let conflict = store
        .edit("note.md", "alpha", "beta", &"cd".repeat(32))
        .unwrap();
    assert_eq!(status(&conflict), "conflict");
    assert_eq!(
        fs::read_to_string(root.join("note.md")).unwrap(),
        "alpha alpha"
    );
    assert_eq!(
        execution(
            store
                .edit("note.md", "missing", "beta", &revision)
                .unwrap_err()
        ),
        "old_text must match exactly once"
    );
    assert_eq!(
        execution(
            store
                .edit("note.md", "alpha", "beta", &revision)
                .unwrap_err()
        ),
        "old_text must match exactly once"
    );
    assert_eq!(
        fs::read_to_string(root.join("note.md")).unwrap(),
        "alpha alpha"
    );
    store.write("once.md", "alpha", None).unwrap();
    let once = store.read("once.md", None, None).unwrap();
    let once_revision = once["revision"].as_str().unwrap();
    let oversized = "x".repeat(MAX_WORKBENCH_FILE_BYTES + 1);
    assert_eq!(
        execution(
            store
                .edit("once.md", "alpha", &oversized, once_revision)
                .unwrap_err()
        ),
        "Edited workbench file is too large"
    );
    assert_eq!(fs::read_to_string(root.join("once.md")).unwrap(), "alpha");
    let edited = store
        .edit("once.md", "alpha", "beta", once_revision)
        .unwrap();
    assert_eq!(status(&edited), "ok");
    assert_eq!(fs::read_to_string(root.join("once.md")).unwrap(), "beta");
}

#[test]
fn workbench_same_revision_mutations_commit_exactly_one_write() {
    let temporary = tempfile::tempdir().unwrap();
    let (store, root) = store(&temporary);
    let created = store.write("race.md", "seed", None).unwrap();
    let revision = created["revision"].as_str().unwrap().to_owned();
    let wins = Arc::new(AtomicUsize::new(0));
    let conflicts = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|content| {
            let store = store.clone();
            let revision = revision.clone();
            let wins = wins.clone();
            let conflicts = conflicts.clone();
            thread::spawn(move || {
                let result = store.write("race.md", content, Some(&revision)).unwrap();
                match status(&result) {
                    "ok" => {
                        wins.fetch_add(1, Ordering::SeqCst);
                    }
                    "conflict" => {
                        conflicts.fetch_add(1, Ordering::SeqCst);
                    }
                    other => panic!("unexpected status {other}"),
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(wins.load(Ordering::SeqCst), 1);
    assert_eq!(conflicts.load(Ordering::SeqCst), 1);
    let live = fs::read_to_string(root.join("race.md")).unwrap();
    assert!(live == "first" || live == "second", "live bytes {live}");
    let leftover = fs::read_dir(&root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"));
    assert!(!leftover, "atomic replace left a temporary file");
}

#[test]
fn workbench_failed_replace_does_not_show_a_partial_file() {
    let temporary = tempfile::tempdir().unwrap();
    let (store, root) = store(&temporary);
    store.write("note.md", "original", None).unwrap();
    assert_eq!(
        execution(
            store
                .write("note.md", &"x".repeat(MAX_WORKBENCH_FILE_BYTES + 1), None)
                .unwrap_err()
        ),
        "Workbench file is too large"
    );
    assert_eq!(
        fs::read_to_string(root.join("note.md")).unwrap(),
        "original"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let created = store.write("locked/note.md", "original", None).unwrap();
        let revision = created["revision"].as_str().unwrap().to_owned();
        let parent = root.join("locked");
        let previous = fs::metadata(&parent).unwrap().permissions();
        let mut denied = previous.clone();
        denied.set_mode(0o500);
        fs::set_permissions(&parent, denied).unwrap();
        let result = store.write("locked/note.md", "partial", Some(&revision));
        fs::set_permissions(&parent, previous).unwrap();
        assert!(result.is_err(), "read-only parent must reject the replace");
        assert_eq!(
            fs::read_to_string(parent.join("note.md")).unwrap(),
            "original"
        );
        let leftover = fs::read_dir(&parent)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name().to_string_lossy().contains(".tmp"));
        assert!(!leftover);
    }
}

#[test]
fn workbench_rejects_attachment_symlink_escape_and_non_utf8() {
    let temporary = tempfile::tempdir().unwrap();
    let (store, root) = store(&temporary);
    let attachment = root.join("attachments/chat-a/message-a");
    fs::create_dir_all(&attachment).unwrap();
    fs::write(attachment.join("file"), "immutable attachment").unwrap();
    let read = store
        .read("attachments/chat-a/message-a/file", None, None)
        .unwrap();
    assert_eq!(read["content"], "immutable attachment");
    assert_eq!(
        execution(
            store
                .write("attachments/chat-a/message-a/file", "overwritten", None)
                .unwrap_err()
        ),
        "App-owned attachment snapshots are read-only"
    );
    assert_eq!(
        execution(
            store
                .write("attachments/chat-a/message-a/injected", "injected", None)
                .unwrap_err()
        ),
        "App-owned attachment snapshots are read-only"
    );
    assert_eq!(
        fs::read_to_string(attachment.join("file")).unwrap(),
        "immutable attachment"
    );
    assert_eq!(
        execution(store.read("../outside.txt", None, None).unwrap_err()),
        "Path is outside this Guru's workbench"
    );
    assert_eq!(
        execution(
            store
                .read(root.join("inside.txt").to_str().unwrap(), None, None)
                .unwrap_err()
        ),
        "Workbench path must be relative"
    );
    assert_eq!(
        execution(store.write("/etc/passwd", "no", None).unwrap_err()),
        "Workbench path must be relative"
    );

    #[cfg(unix)]
    {
        let outside = temporary.path().join("outside.txt");
        fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link.md")).unwrap();
        assert_eq!(
            execution(store.read("link.md", None, None).unwrap_err()),
            "Path escapes this Guru's workbench through a symlink"
        );
        fs::write(root.join("inside.md"), "inside").unwrap();
        std::os::unix::fs::symlink(root.join("inside.md"), root.join("inner-link.md")).unwrap();
        assert_eq!(
            execution(store.read("inner-link.md", None, None).unwrap_err()),
            "Workbench tools do not follow symbolic links"
        );
        let escaped = temporary.path().join("escaped");
        fs::create_dir(&escaped).unwrap();
        std::os::unix::fs::symlink(&escaped, root.join("out")).unwrap();
        assert_eq!(
            execution(store.write("out/file.md", "no", None).unwrap_err()),
            "Path escapes this Guru's workbench through a symlink"
        );
    }

    fs::write(root.join("binary.md"), [0xff, 0xfe]).unwrap();
    assert_eq!(
        execution(store.read("binary.md", None, None).unwrap_err()),
        "Workbench file is not a bounded regular file"
    );
}

#[cfg(unix)]
#[test]
fn workbench_atomic_replace_propagates_directory_fsync_errors() {
    let temporary = tempfile::tempdir().unwrap();
    let (store, root) = store(&temporary);
    FORCE_DIRECTORY_FSYNC_ERROR.with(|flag| flag.set(true));
    let result = store.write("note.md", "durable", None);
    FORCE_DIRECTORY_FSYNC_ERROR.with(|flag| flag.set(false));
    assert_eq!(
        execution(result.unwrap_err()),
        io::Error::from_raw_os_error(libc::EIO).to_string()
    );
    assert_eq!(fs::read_to_string(root.join("note.md")).unwrap(), "durable");
}

#[test]
fn workbench_replace_without_a_file_is_not_a_create() {
    let temporary = tempfile::tempdir().unwrap();
    let (store, _) = store(&temporary);
    assert_eq!(
        execution(
            store
                .write("missing.md", "no", Some(&"ab".repeat(32)))
                .unwrap_err()
        ),
        "Workbench path does not exist"
    );
}
