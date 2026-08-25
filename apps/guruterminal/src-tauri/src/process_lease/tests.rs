#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix {
    use super::super::*;
    use std::{
        os::unix::{fs::OpenOptionsExt, process::CommandExt},
        process::{Command, Stdio},
    };

    const HELPER_DIRECTORY_ENV: &str = "GURUTERMINAL_LEASE_HELPER_DIRECTORY";
    const HELPER_READY_ENV: &str = "GURUTERMINAL_LEASE_HELPER_READY";

    fn spawn_sleep() -> std::process::Child {
        let mut command = Command::new("/bin/sleep");
        command
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        command.spawn().unwrap()
    }

    fn kill_and_reap(child: &mut std::process::Child, process_group_id: i32) {
        signal_process_group(process_group_id, libc::SIGKILL).unwrap();
        child.wait().unwrap();
        assert!(wait_for_group_exit_blocking(process_group_id, Duration::from_secs(2)).unwrap());
    }

    #[tokio::test]
    async fn bounded_group_exit_confirmation_keeps_live_groups_unconfirmed() {
        let mut child = spawn_sleep();
        let process_group_id = child.id() as i32;

        let outcome = confirm_process_group_exit(process_group_id, Duration::from_millis(10)).await;
        assert!(matches!(outcome, ProcessGroupTermination::Unconfirmed));

        kill_and_reap(&mut child, process_group_id);
    }

    #[tokio::test]
    async fn exited_group_has_a_confirmed_exit_observation() {
        let mut child = spawn_sleep();
        let process_group_id = child.id() as i32;

        kill_and_reap(&mut child, process_group_id);
        let outcome = confirm_process_group_exit(process_group_id, Duration::from_millis(10)).await;
        assert!(
            outcome.is_confirmed(),
            "unexpected exit observation: {outcome:?}"
        );
    }

    #[test]
    fn identity_mismatch_never_kills_the_live_process() {
        let temporary = tempfile::tempdir().unwrap();
        let lease_directory = temporary.path().join("leases");
        let mut child = spawn_sleep();
        let pid = child.id() as i32;
        let lease = ChildProcessLease::register(
            &lease_directory,
            ProcessKind::Pi,
            pid,
            pid,
            Path::new("/bin/sleep"),
        )
        .unwrap();

        let lease_path = lease_directory.join(&lease.file_name);
        let mut record: ProcessLeaseRecord =
            serde_json::from_slice(&fs::read(&lease_path).unwrap()).unwrap();
        record.start.primary = record.start.primary.saturating_add(1);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&lease_path)
            .unwrap();
        file.write_all(&serde_json::to_vec(&record).unwrap())
            .unwrap();
        file.sync_all().unwrap();

        assert!(matches!(
            recover_orphaned_processes(&lease_directory),
            Err(ProcessLeaseError::IdentityMismatch)
        ));
        assert!(child.try_wait().unwrap().is_none());
        kill_and_reap(&mut child, pid);
        lease.complete().unwrap();
    }

    #[tokio::test]
    async fn cancelled_sidecars_are_reaped_without_exhausting_lease_capacity() {
        let temporary = tempfile::tempdir().unwrap();
        let lease_directory = temporary.path().join("leases");
        for _ in 0..(MAX_LEASE_ENTRIES + 4) {
            let mut child = spawn_sleep();
            let process_group_id = child.id() as i32;
            let lease = ChildProcessLease::register(
                &lease_directory,
                ProcessKind::Pi,
                process_group_id,
                process_group_id,
                Path::new("/bin/sleep"),
            )
            .unwrap();
            terminate_and_reap_process_group(process_group_id, lease);
            let _ = child.wait();
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if fs::read_dir(&lease_directory).unwrap().count() == 0 {
                        break;
                    }
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            })
            .await
            .unwrap();
        }
    }

    #[test]
    fn lease_reads_are_bounded_and_do_not_follow_symlinks() {
        let temporary = tempfile::tempdir().unwrap();
        let lease_directory = temporary.path().join("leases");
        prepare_lease_directory(&lease_directory).unwrap();
        let nonce = "a".repeat(32);
        let target = temporary.path().join("outside");
        fs::write(&target, b"not a lease").unwrap();
        std::os::unix::fs::symlink(&target, lease_directory.join(lease_file_name(&nonce))).unwrap();
        assert!(matches!(
            recover_orphaned_processes(&lease_directory),
            Err(ProcessLeaseError::UnsafeFilesystemEntry)
        ));
        assert_eq!(fs::read(&target).unwrap(), b"not a lease");

        fs::remove_file(lease_directory.join(lease_file_name(&nonce))).unwrap();
        let oversized = lease_directory.join(lease_file_name(&"b".repeat(32)));
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(oversized)
            .unwrap();
        file.write_all(&vec![b'x'; MAX_LEASE_BYTES as usize + 1])
            .unwrap();
        file.sync_all().unwrap();
        assert!(matches!(
            recover_orphaned_processes(&lease_directory),
            Err(ProcessLeaseError::LimitExceeded)
        ));
    }

    #[test]
    fn forced_parent_crash_is_recovered_on_the_next_start() {
        let temporary = tempfile::tempdir().unwrap();
        let lease_directory = temporary.path().join("leases");
        let ready = temporary.path().join("ready");
        let helper_name = "process_lease::tests::unix::crash_parent_helper";
        let mut helper = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", helper_name, "--nocapture"])
            .env(HELPER_DIRECTORY_ENV, &lease_directory)
            .env(HELPER_READY_ENV, &ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() && Instant::now() < deadline {
            std::thread::sleep(POLL_INTERVAL);
        }
        assert!(ready.exists(), "crash helper did not register its lease");
        let process_group_id: i32 = fs::read_to_string(&ready).unwrap().parse().unwrap();
        assert!(!helper.wait().unwrap().success());

        recover_orphaned_processes(&lease_directory).unwrap();
        assert_ne!(
            process_group_state(process_group_id).unwrap(),
            GroupState::Live
        );
        assert_eq!(fs::read_dir(&lease_directory).unwrap().count(), 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parallel_group_disappearance_stress_completes_every_lease() {
        const WORKERS: usize = 2;
        const ITERATIONS: usize = 128;

        std::thread::scope(|scope| {
            let mut workers = Vec::new();
            for _ in 0..WORKERS {
                workers.push(scope.spawn(|| {
                    let temporary = tempfile::tempdir().unwrap();
                    let lease_directory = temporary.path().join("leases");
                    for _ in 0..ITERATIONS {
                        let mut child = spawn_sleep();
                        let process_group_id = child.id() as i32;
                        let lease = ChildProcessLease::register(
                            &lease_directory,
                            ProcessKind::Pi,
                            process_group_id,
                            process_group_id,
                            Path::new("/bin/sleep"),
                        )
                        .unwrap();
                        signal_process_group(process_group_id, libc::SIGTERM).unwrap();
                        child.wait().unwrap();
                        assert!(
                            wait_for_group_exit_blocking(process_group_id, Duration::from_secs(2),)
                                .unwrap(),
                            "process group {process_group_id} did not exit"
                        );
                        lease.complete().unwrap();
                    }
                    assert_eq!(fs::read_dir(&lease_directory).unwrap().count(), 0);
                }));
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
    }

    #[test]
    fn crash_parent_helper() {
        let (Ok(directory), Ok(ready)) = (
            std::env::var(HELPER_DIRECTORY_ENV),
            std::env::var(HELPER_READY_ENV),
        ) else {
            return;
        };
        let child = spawn_sleep();
        let pid = child.id() as i32;
        let _lease = ChildProcessLease::register(
            Path::new(&directory),
            ProcessKind::Finance,
            pid,
            pid,
            Path::new("/bin/sleep"),
        )
        .unwrap();
        let ready = Path::new(&ready);
        let pending_marker = ready.with_extension(format!("{pid}.tmp"));
        let mut marker = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&pending_marker)
            .unwrap();
        write!(marker, "{pid}").unwrap();
        marker.sync_all().unwrap();
        drop(marker);
        fs::rename(pending_marker, ready).unwrap();

        // SAFETY: this helper is a disposable subprocess created specifically
        // to emulate an ungraceful parent crash.
        unsafe { libc::kill(libc::getpid(), libc::SIGKILL) };
        std::process::abort();
    }
}

#[cfg(windows)]
mod windows_tests {
    use super::super::*;
    use std::{process::Stdio, time::Duration};
    use tokio::{process::Command, time::timeout};

    #[tokio::test]
    async fn job_object_owns_and_terminates_the_suspended_child() {
        let mut command = Command::new("cmd.exe");
        command
            .args(["/D", "/S", "/C", "ping -n 30 127.0.0.1 > nul"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        ChildProcessJob::configure_command(&mut command);
        let mut child = command.spawn().unwrap();
        let job = ChildProcessJob::assign(&child).unwrap();
        assert!(child.try_wait().unwrap().is_none());

        job.terminate_and_wait(Duration::from_secs(2))
            .await
            .unwrap();
        timeout(Duration::from_secs(2), child.wait())
            .await
            .expect("Job Object did not terminate its child")
            .unwrap();
    }

    #[test]
    fn orphan_recovery_is_a_noop_after_job_object_cleanup() {
        let temporary = tempfile::tempdir().unwrap();
        let leases = temporary.path().join("process-leases");
        recover_orphaned_processes(&leases).unwrap();
        assert!(leases.is_dir());
    }
}
