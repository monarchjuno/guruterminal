#!/usr/bin/env python3
"""Smoke staged workers and fail if a launched process group survives."""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import os
import platform
import queue
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
from pathlib import Path, PurePosixPath
from typing import IO, Any

if os.name == "nt":
    from ctypes import wintypes


TIMEOUT_SECONDS = 30.0
CLEANUP_TIMEOUT_SECONDS = 5.0
WINDOWS = os.name == "nt"
APP_ROOT = Path(__file__).resolve().parent.parent
MCP_PROTOCOL_VERSION = "2025-06-18"
OPENBB_CONTROL_TOOLS = {
    "activate_category",
    "activate_tools",
    "available_categories",
    "available_tools",
    "deactivate_tools",
}
OPENBB_LICENSE_DIRECTORY = "THIRD_PARTY_LICENSES"
OPENBB_LICENSE_MANIFEST = "python-distributions.json"
OPENBB_LICENSE_SCHEMA = "guruterminal-python-licenses/1"


if WINDOWS:
    CREATE_SUSPENDED = 0x00000004
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000
    JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION_CLASS = 1
    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS = 9
    PROCESS_SYNCHRONIZE = 0x00100000
    WAIT_OBJECT_0 = 0x00000000

    class JobObjectBasicLimitInformation(ctypes.Structure):
        _fields_ = [
            ("per_process_user_time_limit", ctypes.c_longlong),
            ("per_job_user_time_limit", ctypes.c_longlong),
            ("limit_flags", wintypes.DWORD),
            ("minimum_working_set_size", ctypes.c_size_t),
            ("maximum_working_set_size", ctypes.c_size_t),
            ("active_process_limit", wintypes.DWORD),
            ("affinity", ctypes.c_size_t),
            ("priority_class", wintypes.DWORD),
            ("scheduling_class", wintypes.DWORD),
        ]

    class IoCounters(ctypes.Structure):
        _fields_ = [
            ("read_operation_count", ctypes.c_ulonglong),
            ("write_operation_count", ctypes.c_ulonglong),
            ("other_operation_count", ctypes.c_ulonglong),
            ("read_transfer_count", ctypes.c_ulonglong),
            ("write_transfer_count", ctypes.c_ulonglong),
            ("other_transfer_count", ctypes.c_ulonglong),
        ]

    class JobObjectExtendedLimitInformation(ctypes.Structure):
        _fields_ = [
            ("basic_limit_information", JobObjectBasicLimitInformation),
            ("io_info", IoCounters),
            ("process_memory_limit", ctypes.c_size_t),
            ("job_memory_limit", ctypes.c_size_t),
            ("peak_process_memory_used", ctypes.c_size_t),
            ("peak_job_memory_used", ctypes.c_size_t),
        ]

    class JobObjectBasicAccountingInformation(ctypes.Structure):
        _fields_ = [
            ("total_user_time", ctypes.c_longlong),
            ("total_kernel_time", ctypes.c_longlong),
            ("this_period_total_user_time", ctypes.c_longlong),
            ("this_period_total_kernel_time", ctypes.c_longlong),
            ("total_page_fault_count", wintypes.DWORD),
            ("total_processes", wintypes.DWORD),
            ("active_processes", wintypes.DWORD),
            ("total_terminated_processes", wintypes.DWORD),
        ]

    KERNEL32 = ctypes.WinDLL("kernel32", use_last_error=True)
    KERNEL32.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
    KERNEL32.CreateJobObjectW.restype = wintypes.HANDLE
    KERNEL32.SetInformationJobObject.argtypes = [
        wintypes.HANDLE,
        ctypes.c_int,
        ctypes.c_void_p,
        wintypes.DWORD,
    ]
    KERNEL32.SetInformationJobObject.restype = wintypes.BOOL
    KERNEL32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
    KERNEL32.AssignProcessToJobObject.restype = wintypes.BOOL
    KERNEL32.QueryInformationJobObject.argtypes = [
        wintypes.HANDLE,
        ctypes.c_int,
        ctypes.c_void_p,
        wintypes.DWORD,
        ctypes.c_void_p,
    ]
    KERNEL32.QueryInformationJobObject.restype = wintypes.BOOL
    KERNEL32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    KERNEL32.OpenProcess.restype = wintypes.HANDLE
    KERNEL32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    KERNEL32.WaitForSingleObject.restype = wintypes.DWORD
    KERNEL32.CloseHandle.argtypes = [wintypes.HANDLE]
    KERNEL32.CloseHandle.restype = wintypes.BOOL
    NTDLL = ctypes.WinDLL("ntdll")
    NTDLL.NtResumeProcess.argtypes = [wintypes.HANDLE]
    NTDLL.NtResumeProcess.restype = ctypes.c_long


class WindowsProcessJob:
    """Own a Windows process tree and kill every descendant when closed."""

    def __init__(self, process: subprocess.Popen[str]) -> None:
        if not WINDOWS:
            raise RuntimeError("Windows process jobs are unavailable")
        self.handle = KERNEL32.CreateJobObjectW(None, None)
        if not self.handle:
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            limits = JobObjectExtendedLimitInformation()
            limits.basic_limit_information.limit_flags = (
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            )
            if not KERNEL32.SetInformationJobObject(
                self.handle,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                ctypes.byref(limits),
                ctypes.sizeof(limits),
            ):
                raise ctypes.WinError(ctypes.get_last_error())
            if not KERNEL32.AssignProcessToJobObject(
                self.handle, wintypes.HANDLE(int(process._handle))
            ):
                raise ctypes.WinError(ctypes.get_last_error())
            status = NTDLL.NtResumeProcess(wintypes.HANDLE(int(process._handle)))
            if status != 0:
                raise OSError(
                    f"NtResumeProcess failed with NTSTATUS 0x{status & 0xFFFFFFFF:08x}"
                )
        except BaseException:
            KERNEL32.CloseHandle(self.handle)
            self.handle = None
            raise

    def active_processes(self) -> int:
        if self.handle is None:
            return 0
        accounting = JobObjectBasicAccountingInformation()
        if not KERNEL32.QueryInformationJobObject(
            self.handle,
            JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION_CLASS,
            ctypes.byref(accounting),
            ctypes.sizeof(accounting),
            None,
        ):
            raise ctypes.WinError(ctypes.get_last_error())
        return int(accounting.active_processes)

    def close(self, *, expect_empty: bool) -> None:
        if self.handle is None:
            return
        active = 0
        error: BaseException | None = None
        if expect_empty:
            deadline = time.monotonic() + 2.0
            try:
                while True:
                    active = self.active_processes()
                    if active == 0 or time.monotonic() >= deadline:
                        break
                    time.sleep(0.05)
            except BaseException as caught:
                error = caught
        handle = self.handle
        self.handle = None
        if not KERNEL32.CloseHandle(handle) and error is None:
            error = ctypes.WinError(ctypes.get_last_error())
        if error is not None:
            raise error
        if expect_empty and active != 0:
            raise RuntimeError(
                f"orphaned Windows job contained {active} surviving process(es)"
            )


def expected_finance_identity() -> tuple[str, str]:
    """Read the exact source identity that the staged finance worker must report."""
    python_root = APP_ROOT / "python"
    pyproject = tomllib.loads(
        (python_root / "pyproject.toml").read_text(encoding="utf-8")
    )
    worker_version = pyproject.get("project", {}).get("version")
    if not isinstance(worker_version, str) or not worker_version.strip():
        raise RuntimeError("finance worker project version is missing")
    lock_digest = hashlib.sha256((python_root / "uv.lock").read_bytes()).hexdigest()
    return worker_version, lock_digest


def expected_openbb_identity() -> tuple[dict[str, Any], bytes]:
    """Read the source manifest and lockfile that the frozen runtime must retain."""
    openbb_root = APP_ROOT / "openbb"
    manifest = json.loads(
        (openbb_root / "runtime-manifest.json").read_text(encoding="utf-8")
    )
    if not isinstance(manifest, dict):
        raise RuntimeError("OpenBB runtime manifest is not an object")
    lock_bytes = (openbb_root / "uv.lock").read_bytes()
    lock_digest = hashlib.sha256(lock_bytes).hexdigest()
    if manifest.get("uv_lock_sha256") != lock_digest:
        raise RuntimeError("OpenBB source manifest does not match its uv.lock")
    return manifest, lock_bytes


def _canonical_distribution_name(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def validate_openbb_license_archive(
    runtime_dir: Path,
    runtime_manifest: dict[str, Any],
    lock_bytes: bytes,
) -> None:
    """Verify every byte in the frozen runtime's separate legal archive."""

    archive_root = runtime_dir / OPENBB_LICENSE_DIRECTORY
    manifest_path = archive_root / OPENBB_LICENSE_MANIFEST
    if archive_root.is_symlink() or not archive_root.is_dir():
        raise RuntimeError("OpenBB third-party license archive is missing")
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeError("OpenBB third-party license manifest is invalid") from error
    if manifest.get("schema_version") != OPENBB_LICENSE_SCHEMA:
        raise RuntimeError("OpenBB third-party license schema is invalid")
    lock_digest = hashlib.sha256(lock_bytes).hexdigest()
    if manifest.get("uv_lock_sha256") != lock_digest:
        raise RuntimeError("OpenBB third-party license lock digest is invalid")
    archive_platform = manifest.get("platform")
    if (
        not isinstance(archive_platform, dict)
        or archive_platform.get("system") != sys.platform
        or str(archive_platform.get("machine", "")).lower()
        != platform.machine().lower()
    ):
        raise RuntimeError("OpenBB third-party license platform is invalid")

    file_records = manifest.get("files")
    if not isinstance(file_records, list) or not file_records:
        raise RuntimeError("OpenBB third-party license file inventory is empty")
    records_by_path: dict[str, dict[str, Any]] = {}
    for record in file_records:
        if not isinstance(record, dict):
            raise RuntimeError("OpenBB third-party license file record is invalid")
        relative = PurePosixPath(str(record.get("path", "")))
        if relative.is_absolute() or not relative.parts or ".." in relative.parts:
            raise RuntimeError("OpenBB third-party license path is unsafe")
        relative_name = relative.as_posix()
        if relative_name in records_by_path or relative.name == "direct_url.json":
            raise RuntimeError("OpenBB third-party license path is invalid")
        packaged = archive_root.joinpath(*relative.parts)
        if packaged.is_symlink() or not packaged.is_file():
            raise RuntimeError(
                f"OpenBB third-party license file is missing: {relative_name}"
            )
        if packaged.stat().st_size != record.get("size") or hashlib.sha256(
            packaged.read_bytes()
        ).hexdigest() != record.get("sha256"):
            raise RuntimeError(
                f"OpenBB third-party license digest is invalid: {relative_name}"
            )
        records_by_path[relative_name] = record

    actual_paths: set[str] = set()
    for path in archive_root.rglob("*"):
        if path.is_symlink():
            raise RuntimeError("OpenBB third-party license archive contains a symlink")
        if path.is_file() and path != manifest_path:
            actual_paths.add(path.relative_to(archive_root).as_posix())
    if actual_paths != set(records_by_path):
        raise RuntimeError("OpenBB third-party license file inventory differs")

    try:
        locked = tomllib.loads(lock_bytes.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise RuntimeError("OpenBB source lockfile is invalid") from error
    locked_versions: dict[str, set[str]] = {}
    for package in locked.get("package", []):
        name = _canonical_distribution_name(str(package.get("name", "")))
        version = str(package.get("version", ""))
        if name and version:
            locked_versions.setdefault(name, set()).add(version)

    distributions = manifest.get("distributions")
    if not isinstance(distributions, list) or len(distributions) < 100:
        raise RuntimeError("OpenBB third-party distribution inventory is incomplete")
    names = []
    distributions_by_name: dict[str, dict[str, Any]] = {}
    for entry in distributions:
        if not isinstance(entry, dict):
            raise RuntimeError("OpenBB third-party distribution entry is invalid")
        name = _canonical_distribution_name(str(entry.get("name", "")))
        version = str(entry.get("version", ""))
        if (
            not name
            or name in distributions_by_name
            or version not in locked_versions.get(name, set())
        ):
            raise RuntimeError("OpenBB third-party distribution identity is invalid")
        archive_files = entry.get("archive_files")
        license_files = entry.get("license_files")
        if (
            not isinstance(archive_files, list)
            or not archive_files
            or not isinstance(license_files, list)
            or not license_files
            or not set(archive_files).issubset(records_by_path)
            or not set(license_files).issubset(records_by_path)
        ):
            raise RuntimeError(
                f"OpenBB third-party license entry is incomplete: {name}"
            )
        for declared in entry.get("declared_license_files", []):
            if (
                not isinstance(declared, dict)
                or not isinstance(declared.get("files"), list)
                or not declared["files"]
                or not set(declared["files"]).issubset(records_by_path)
            ):
                raise RuntimeError(
                    f"OpenBB declared License-File is unresolved: {name}"
                )
        names.append(name)
        distributions_by_name[name] = entry
    if names != sorted(names):
        raise RuntimeError("OpenBB third-party distribution inventory is not sorted")

    required_packages = {
        "certifi",
        "cryptography",
        "numpy",
        "openbb",
        "scipy",
        *(
            _canonical_distribution_name(str(name))
            for name in runtime_manifest.get("packages", {})
        ),
        *(
            _canonical_distribution_name(str(provider.get("package", "")))
            for provider in runtime_manifest.get("providers", [])
            if isinstance(provider, dict)
        ),
    }
    if not required_packages.issubset(distributions_by_name):
        raise RuntimeError("OpenBB third-party license archive omits a runtime package")

    repository_license = APP_ROOT.parent.parent / "LICENSE"
    repository_license_digest = hashlib.sha256(
        repository_license.read_bytes()
    ).hexdigest()
    for name, entry in distributions_by_name.items():
        if name != "openbb" and not name.startswith("openbb-"):
            continue
        if entry.get("effective_license") != "AGPL-3.0-only" or not any(
            records_by_path[path].get("sha256") == repository_license_digest
            for path in entry["license_files"]
        ):
            raise RuntimeError(f"OpenBB AGPL license text is missing: {name}")

    toolchain = manifest.get("toolchain")
    if not isinstance(toolchain, list):
        raise RuntimeError("OpenBB toolchain license inventory is invalid")
    toolchain_by_name = {
        entry.get("name"): entry for entry in toolchain if isinstance(entry, dict)
    }
    if set(toolchain_by_name) != {"cpython", "pyinstaller"}:
        raise RuntimeError("OpenBB toolchain license inventory is incomplete")
    for name, entry in toolchain_by_name.items():
        license_files = entry.get("license_files")
        if (
            not isinstance(license_files, list)
            or not license_files
            or not set(license_files).issubset(records_by_path)
        ):
            raise RuntimeError(f"OpenBB toolchain license is incomplete: {name}")


def smoke_environment() -> dict[str, str]:
    """Return the non-secret environment shared by packaged sidecar smoke tests."""
    environment = {
        "LANG": "C.UTF-8",
        "PATH": os.environ.get("PATH", os.defpath),
    }
    platform_names = ("SYSTEMROOT", "WINDIR", "TEMP", "TMP") if WINDOWS else ("TMPDIR",)
    for name in platform_names:
        if value := os.environ.get(name):
            environment[name] = value
    return environment


def finance_environment() -> dict[str, str]:
    environment = smoke_environment()
    environment.update(
        {
            "PYTHONHASHSEED": "0",
            "PYTHONDONTWRITEBYTECODE": "1",
            "PYTHONNOUSERSITE": "1",
        }
    )
    return environment


def openbb_environment() -> dict[str, str]:
    environment = smoke_environment()
    environment.update(
        {
            "PYTHONHASHSEED": "0",
            "PYTHONDONTWRITEBYTECODE": "1",
            "PYTHONNOUSERSITE": "1",
        }
    )
    return environment


def pi_environment(runtime_dir: Path) -> dict[str, str]:
    environment = smoke_environment()
    environment.update(
        {
            "PI_OFFLINE": "1",
            "PI_PACKAGE_DIR": str(runtime_dir),
            "PI_TELEMETRY": "0",
        }
    )
    return environment


def spawn_options() -> dict[str, Any]:
    if WINDOWS:
        return {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP | CREATE_SUSPENDED}
    return {"start_new_session": True}


def process_group_exists(process_group: int) -> bool:
    if WINDOWS:
        result = subprocess.run(
            [
                "tasklist",
                "/FI",
                f"PID eq {process_group}",
                "/FO",
                "CSV",
                "/NH",
            ],
            check=False,
            capture_output=True,
            env=smoke_environment(),
            text=True,
            timeout=CLEANUP_TIMEOUT_SECONDS,
        )
        return f'"{process_group}"' in result.stdout
    try:
        os.killpg(process_group, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def wait_for_group_exit(process_group: int) -> None:
    deadline = time.monotonic() + 2.0
    while time.monotonic() < deadline:
        if not process_group_exists(process_group):
            return
        time.sleep(0.05)
    raise RuntimeError(f"orphaned process group detected: {process_group}")


def cleanup(process: subprocess.Popen[str], process_group: int) -> None:
    if WINDOWS:
        taskkill_timeout: subprocess.TimeoutExpired | None = None
        if process.poll() is None:
            try:
                subprocess.run(
                    ["taskkill", "/PID", str(process_group), "/T", "/F"],
                    check=False,
                    capture_output=True,
                    env=smoke_environment(),
                    timeout=CLEANUP_TIMEOUT_SECONDS,
                )
            except subprocess.TimeoutExpired as error:
                taskkill_timeout = error
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=2)
        if taskkill_timeout is not None:
            raise RuntimeError(
                f"taskkill exceeded the {CLEANUP_TIMEOUT_SECONDS:g}-second cleanup limit"
            ) from taskkill_timeout
        return
    if process_group_exists(process_group):
        try:
            os.killpg(process_group, signal.SIGTERM)
        except ProcessLookupError:
            pass
    if process.poll() is None:
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            if process_group_exists(process_group):
                os.killpg(process_group, signal.SIGKILL)
            process.wait(timeout=2)
    if process_group_exists(process_group):
        os.killpg(process_group, signal.SIGKILL)
    wait_for_group_exit(process_group)


def own_process_tree(
    process: subprocess.Popen[str], process_group: int
) -> WindowsProcessJob | None:
    if not WINDOWS:
        return None
    try:
        return WindowsProcessJob(process)
    except BaseException as error:
        try:
            cleanup(process, process_group)
        except BaseException as cleanup_error:
            error.add_note(f"process cleanup also failed: {cleanup_error}")
        raise


def finish_process_tree(
    job: WindowsProcessJob | None,
    process_group: int,
    *,
    expect_empty: bool,
) -> None:
    if job is not None:
        job.close(expect_empty=expect_empty)
    else:
        wait_for_group_exit(process_group)


def finish_smoke_process(
    process: subprocess.Popen[str],
    process_group: int,
    job: WindowsProcessJob | None,
    *,
    completed: bool,
) -> None:
    """Prove successful workers drained; best-effort kill failed workers."""
    if not completed:
        cleanup_error = cleanup_owned_process(process, process_group, job)
        active_error = sys.exception()
        if cleanup_error is not None and active_error is not None:
            active_error.add_note(f"process-tree cleanup also failed: {cleanup_error}")
        elif cleanup_error is not None:
            raise cleanup_error
        elif active_error is None:
            raise RuntimeError("smoke process did not reach its completed boundary")
        return

    try:
        finish_process_tree(job, process_group, expect_empty=True)
    except BaseException as error:
        # On Unix, the empty-tree check does not mutate the group. Always reap
        # the detected orphan while preserving the authoritative smoke error.
        cleanup_error = cleanup_owned_process(process, process_group, job)
        if cleanup_error is not None:
            error.add_note(f"process-tree cleanup also failed: {cleanup_error}")
        raise


def cleanup_owned_process(
    process: subprocess.Popen[str],
    process_group: int,
    job: WindowsProcessJob | None,
) -> BaseException | None:
    """Bound cleanup and always close the Job Object, retaining the first error."""
    error: BaseException | None = None
    try:
        cleanup(process, process_group)
    except BaseException as caught:
        error = caught
    finally:
        if job is not None:
            try:
                job.close(expect_empty=False)
            except BaseException as caught:
                if error is None:
                    error = caught
                else:
                    error.add_note(f"Job Object close also failed: {caught}")
    return error


def stop_root_process(process: subprocess.Popen[str]) -> None:
    """Stop only the long-running root so descendants remain observable."""
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=2)


def windows_job_self_test_child() -> int:
    """Remain alive long enough for the parent-first-exit fixture to inspect us."""
    if not WINDOWS:
        raise RuntimeError("Windows Job Object self-test is Windows-only")
    time.sleep(TIMEOUT_SECONDS)
    return 0


def windows_job_self_test_root(marker: Path) -> int:
    """Spawn a child and exit without waiting, reproducing the launcher race."""
    if not WINDOWS:
        raise RuntimeError("Windows Job Object self-test is Windows-only")
    child = subprocess.Popen(
        [
            sys.executable,
            str(Path(__file__).resolve()),
            "--windows-job-self-test-child",
        ],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        env=smoke_environment(),
    )
    marker.write_text(f"{child.pid}\n", encoding="utf-8")
    return 0


def windows_job_self_test() -> int:
    """Prove a descendant survives its root but not the owned Job Object."""
    if not WINDOWS:
        raise RuntimeError("Windows Job Object self-test is Windows-only")
    with tempfile.TemporaryDirectory(prefix="guruterminal-job-self-test-") as name:
        marker = Path(name) / "child.pid"
        process = subprocess.Popen(
            [
                sys.executable,
                str(Path(__file__).resolve()),
                "--windows-job-self-test-root",
                str(marker),
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=smoke_environment(),
            **spawn_options(),
        )
        job: WindowsProcessJob | None = None
        child_handle = None
        try:
            job = own_process_tree(process, process.pid)
            stdout, stderr = process.communicate(timeout=CLEANUP_TIMEOUT_SECONDS)
            if process.returncode != 0:
                raise RuntimeError(
                    "Windows Job Object self-test root failed: "
                    f"{(stderr or stdout).strip()}"
                )
            child_pid = int(marker.read_text(encoding="utf-8").strip())
            child_handle = KERNEL32.OpenProcess(PROCESS_SYNCHRONIZE, False, child_pid)
            if not child_handle:
                raise ctypes.WinError(ctypes.get_last_error())
            if job.active_processes() != 1:
                raise RuntimeError(
                    "Windows Job Object did not retain the root's live descendant"
                )

            try:
                finish_smoke_process(process, process.pid, job, completed=True)
            except RuntimeError as error:
                if "orphaned Windows job contained 1 surviving process(es)" not in str(
                    error
                ):
                    raise
            else:
                raise RuntimeError(
                    "Windows Job Object accepted a descendant after its root exited"
                )

            wait_result = KERNEL32.WaitForSingleObject(
                child_handle, int(CLEANUP_TIMEOUT_SECONDS * 1000)
            )
            if wait_result != WAIT_OBJECT_0:
                raise RuntimeError(
                    "kill-on-close Job Object did not terminate the surviving descendant"
                )
        finally:
            cleanup_error: BaseException | None = None
            try:
                cleanup_error = cleanup_owned_process(process, process.pid, job)
            finally:
                if child_handle:
                    KERNEL32.CloseHandle(child_handle)
            active_error = sys.exception()
            if cleanup_error is not None and active_error is not None:
                active_error.add_note(
                    f"Windows Job Object fixture cleanup also failed: {cleanup_error}"
                )
            elif cleanup_error is not None:
                raise cleanup_error
    print("Windows Job Object root-exit descendant fixture passed.")
    return 0


def one_shot_version(
    executable: Path,
    expected: str,
    environment: dict[str, str],
) -> None:
    process = subprocess.Popen(
        [str(executable), "--version"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=environment,
        **spawn_options(),
    )
    process_group = process.pid
    job = own_process_tree(process, process_group)
    completed = False
    try:
        stdout, stderr = process.communicate(timeout=TIMEOUT_SECONDS)
        if process.returncode != 0:
            raise RuntimeError(f"{executable.name} --version failed: {stderr.strip()}")
        if stdout.strip() != expected:
            raise RuntimeError(
                f"{executable.name} returned {stdout.strip()!r}, expected {expected!r}"
            )
        completed = True
    finally:
        finish_smoke_process(process, process_group, job, completed=completed)


def stream_lines(stream: IO[str], lines: queue.Queue[str]) -> None:
    while line := stream.readline():
        lines.put(line)
    lines.put("")


def read_json_line(lines: queue.Queue[str], timeout: float) -> dict[str, Any]:
    try:
        line = lines.get(timeout=timeout)
    except queue.Empty as error:
        raise RuntimeError("finance worker response timed out") from error
    if not line:
        raise RuntimeError("finance worker closed without a response")
    value = json.loads(line)
    if not isinstance(value, dict):
        raise RuntimeError("finance worker response is not an object")
    return value


def read_response(
    lines: queue.Queue[str], request_id: str, timeout: float
) -> tuple[dict[str, Any], bool]:
    deadline = time.monotonic() + timeout
    saw_progress = False
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise RuntimeError(f"finance worker response timed out: {request_id}")
        message = read_json_line(lines, remaining)
        if message.get("method") == "progress":
            params = message.get("params")
            if isinstance(params, dict) and params.get("id") == request_id:
                saw_progress = True
            continue
        if message.get("id") == request_id:
            return message, saw_progress


def send_json(process: subprocess.Popen[str], value: dict[str, Any]) -> None:
    if process.stdin is None:
        raise RuntimeError("finance worker stdin is unavailable")
    process.stdin.write(json.dumps(value, separators=(",", ":")) + "\n")
    process.stdin.flush()


def finance_smoke(
    executable: Path,
    environment: dict[str, str],
    expected_worker_version: str,
    expected_lock_digest: str,
) -> None:
    process = subprocess.Popen(
        [str(executable)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=environment,
        **spawn_options(),
    )
    process_group = process.pid
    job = own_process_tree(process, process_group)
    completed = False
    try:
        send_json(
            process,
            {
                "jsonrpc": "2.0",
                "id": "handshake",
                "method": "system.handshake",
                "params": {
                    "protocol_version": "1",
                    "client": {"name": "package-check", "version": "0.1.0"},
                },
            },
        )
        if process.stdout is None:
            raise RuntimeError("finance worker stdout is unavailable")
        lines: queue.Queue[str] = queue.Queue()
        threading.Thread(
            target=stream_lines,
            args=(process.stdout, lines),
            daemon=True,
        ).start()
        response, _ = read_response(lines, "handshake", TIMEOUT_SECONDS)
        result = response.get("result")
        if response.get("id") != "handshake" or not isinstance(result, dict):
            raise RuntimeError("finance worker handshake failed")
        if result.get("protocol_version") != "1":
            raise RuntimeError("finance worker protocol version mismatch")
        if result.get("worker_version") != expected_worker_version:
            raise RuntimeError("finance worker package version mismatch")
        python_version = result.get("python_version")
        if not isinstance(python_version, str) or not python_version.startswith(
            "3.12."
        ):
            raise RuntimeError("finance worker Python version mismatch")
        if result.get("lock_digest") != expected_lock_digest:
            raise RuntimeError("finance worker uv.lock digest mismatch")
        if set(result.get("tools", [])) != {
            "compound_annual_growth_rate",
            "currency_convert",
            "dcf_sensitivity",
            "discounted_cash_flow",
            "enterprise_value_bridge",
            "internal_rate_of_return",
            "percentage_change",
            "period_aggregate",
            "point_in_time_filter",
            "ratio",
            "risk_metrics",
            "series_statistics",
            "weighted_average_cost_of_capital",
        }:
            raise RuntimeError("finance worker exposed an unexpected tool set")

        send_json(
            process,
            {
                "jsonrpc": "2.0",
                "id": "calculation",
                "method": "tools.call",
                "params": {
                    "name": "percentage_change",
                    "arguments": {"start": "80", "end": "100", "precision": 2},
                    "context": {
                        "data_cutoff": "2025-01-01T00:00:00Z",
                        "timeout_ms": 30000,
                        "sources": [
                            {
                                "source_id": "package-fixture",
                                "provider": "package-check",
                                "as_of": "2024-09-30T00:00:00Z",
                                "available_at": "2024-11-01T00:00:00Z",
                                "retrieved_at": "2025-01-01T00:00:00Z",
                            }
                        ],
                    },
                },
            },
        )
        response, saw_progress = read_response(lines, "calculation", TIMEOUT_SECONDS)
        calculation = response.get("result")
        if (
            not saw_progress
            or not isinstance(calculation, dict)
            or calculation.get("data", {}).get("value") != "25"
            or calculation.get("provenance", {})
            .get("sources", [{}])[0]
            .get("source_id")
            != "package-fixture"
        ):
            raise RuntimeError("packaged finance calculation failed provenance checks")

        send_json(
            process,
            {
                "jsonrpc": "2.0",
                "id": "shutdown",
                "method": "system.shutdown",
                "params": {},
            },
        )
        response, _ = read_response(lines, "shutdown", TIMEOUT_SECONDS)
        if response.get("id") != "shutdown" or response.get("result") != {
            "stopping": True
        }:
            raise RuntimeError("finance worker did not acknowledge shutdown")
        process.wait(timeout=TIMEOUT_SECONDS)
        if process.returncode != 0:
            stderr = process.stderr.read() if process.stderr is not None else ""
            raise RuntimeError(
                f"finance worker exited unsuccessfully: {stderr.strip()}"
            )
        completed = True
    finally:
        finish_smoke_process(process, process_group, job, completed=completed)


def validate_openbb_bundle(executable: Path, runtime_dir: Path) -> dict[str, Any]:
    """Prove the onedir runtime retains the exact reviewed manifest and lockfile."""
    if runtime_dir.name != "openbb":
        raise RuntimeError("OpenBB runtime must be staged at pi-runtime/openbb")
    if runtime_dir.is_symlink() or not runtime_dir.is_dir():
        raise RuntimeError("OpenBB runtime is not a real directory")
    resolved_runtime = runtime_dir.resolve(strict=True)
    resolved_executable = executable.resolve(strict=True)
    if resolved_executable.parent != resolved_runtime:
        raise RuntimeError("OpenBB executable is outside its runtime directory")

    expected_manifest, expected_lock = expected_openbb_identity()
    public_manifest_path = resolved_runtime / "runtime-manifest.json"
    internal_manifest_path = (
        resolved_runtime / "_internal" / "guruterminal_openbb" / "runtime-manifest.json"
    )
    lock_path = resolved_runtime / "uv.lock"
    try:
        public_manifest = json.loads(public_manifest_path.read_text(encoding="utf-8"))
        internal_manifest = json.loads(
            internal_manifest_path.read_text(encoding="utf-8")
        )
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeError("OpenBB packaged runtime manifest is invalid") from error

    expected_public = dict(expected_manifest)
    expected_executable = (
        "guruterminal-openbb.exe" if WINDOWS else "guruterminal-openbb"
    )
    expected_public["executable"] = expected_executable
    if public_manifest != expected_public:
        raise RuntimeError("OpenBB public runtime manifest differs from its source")
    if internal_manifest != expected_manifest:
        raise RuntimeError("OpenBB internal runtime manifest differs from its source")
    if resolved_executable.name != expected_executable:
        raise RuntimeError("OpenBB packaged executable name is invalid")
    try:
        packaged_lock = lock_path.read_bytes()
    except OSError as error:
        raise RuntimeError("OpenBB packaged uv.lock is missing") from error
    if packaged_lock != expected_lock:
        raise RuntimeError("OpenBB packaged uv.lock differs from its source")
    if hashlib.sha256(packaged_lock).hexdigest() != public_manifest.get(
        "uv_lock_sha256"
    ):
        raise RuntimeError("OpenBB packaged uv.lock digest is invalid")
    validate_openbb_license_archive(resolved_runtime, public_manifest, packaged_lock)
    return public_manifest


def openbb_smoke(executable: Path, runtime_dir: Path) -> None:
    """Verify the frozen OpenBB control surface and full provider inventory."""
    manifest = validate_openbb_bundle(executable, runtime_dir)
    with tempfile.TemporaryDirectory(prefix="guruterminal-openbb-smoke-") as name:
        scratch = Path(name).resolve(strict=True)
        if not WINDOWS:
            scratch.chmod(0o700)
        process = subprocess.Popen(
            [str(executable)],
            cwd=runtime_dir,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            env=openbb_environment(),
            **spawn_options(),
        )
        process_group = process.pid
        job = own_process_tree(process, process_group)
        completed = False
        try:
            if process.stdout is None:
                raise RuntimeError("OpenBB MCP stdout is unavailable")
            lines: queue.Queue[str] = queue.Queue()
            threading.Thread(
                target=stream_lines,
                args=(process.stdout, lines),
                daemon=True,
            ).start()
            providers = manifest.get("providers")
            categories = manifest.get("allowed_categories")
            if not isinstance(providers, list) or not isinstance(categories, list):
                raise RuntimeError("OpenBB packaged capability inventory is invalid")
            provider_ids: list[str] = []
            network_hosts: set[str] = set()
            for provider in providers:
                if not isinstance(provider, dict):
                    raise RuntimeError("OpenBB packaged provider inventory is invalid")
                provider_id = provider.get("id")
                hosts = provider.get("network_hosts")
                if (
                    not isinstance(provider_id, str)
                    or not provider_id
                    or not isinstance(hosts, list)
                    or not all(isinstance(host, str) and host for host in hosts)
                ):
                    raise RuntimeError("OpenBB packaged provider inventory is invalid")
                provider_ids.append(provider_id)
                network_hosts.update(hosts)
            if len(set(provider_ids)) != len(provider_ids):
                raise RuntimeError("OpenBB packaged provider inventory has duplicates")
            send_json(
                process,
                {
                    "type": "guruterminal.bootstrap",
                    "protocol_version": 1,
                    "run_id": "package-openbb-smoke",
                    "scratch_dir": str(scratch),
                    "credentials": {},
                    "settings": {
                        "allowed_categories": categories,
                        "enabled_provider_ids": sorted(provider_ids),
                        "allowed_network_hosts": sorted(network_hosts),
                        "provider_config": {},
                    },
                },
            )
            send_json(
                process,
                {
                    "jsonrpc": "2.0",
                    "id": "initialize",
                    "method": "initialize",
                    "params": {
                        "protocolVersion": MCP_PROTOCOL_VERSION,
                        "capabilities": {},
                        "clientInfo": {
                            "name": "Guru Terminal package check",
                            "version": "1.0.0",
                        },
                    },
                },
            )
            initialized, _ = read_response(lines, "initialize", TIMEOUT_SECONDS)
            result = initialized.get("result")
            if (
                not isinstance(result, dict)
                or result.get("protocolVersion") != MCP_PROTOCOL_VERSION
                or not isinstance(result.get("capabilities"), dict)
                or not isinstance(result.get("serverInfo"), dict)
                or not str(result["serverInfo"].get("name", "")).strip()
            ):
                raise RuntimeError("packaged OpenBB MCP initialize failed")
            send_json(
                process,
                {
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized",
                    "params": {},
                },
            )
            send_json(
                process,
                {
                    "jsonrpc": "2.0",
                    "id": "tools-list",
                    "method": "tools/list",
                    "params": {},
                },
            )
            listed, _ = read_response(lines, "tools-list", TIMEOUT_SECONDS)
            tools = listed.get("result", {}).get("tools")
            names = {
                tool.get("name")
                for tool in tools or []
                if isinstance(tool, dict) and isinstance(tool.get("name"), str)
            }
            if not isinstance(tools, list) or names != OPENBB_CONTROL_TOOLS:
                raise RuntimeError(
                    "packaged OpenBB MCP exposed an unexpected initial tool set"
                )

            send_json(
                process,
                {
                    "jsonrpc": "2.0",
                    "id": "available-categories",
                    "method": "tools/call",
                    "params": {"name": "available_categories", "arguments": {}},
                },
            )
            available, _ = read_response(lines, "available-categories", TIMEOUT_SECONDS)
            category_rows = (
                available.get("result", {}).get("structuredContent", {}).get("result")
            )
            discovered_categories = {
                row.get("name")
                for row in category_rows or []
                if isinstance(row, dict) and isinstance(row.get("name"), str)
            }
            if not isinstance(category_rows, list) or discovered_categories != set(
                categories
            ):
                raise RuntimeError(
                    "packaged OpenBB MCP router/category discovery is incomplete"
                )
            for index, category in enumerate(sorted(discovered_categories)):
                request_id = f"activate-category-{index}"
                send_json(
                    process,
                    {
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "method": "tools/call",
                        "params": {
                            "name": "activate_category",
                            "arguments": {"category": category},
                        },
                    },
                )
                activated, _ = read_response(lines, request_id, TIMEOUT_SECONDS)
                if activated.get("result", {}).get("isError") is True:
                    raise RuntimeError(
                        f"packaged OpenBB MCP could not activate category: {category}"
                    )

            inventory: list[dict[str, Any]] = []
            cursor: str | None = None
            seen_cursors: set[str] = set()
            for page_index in range(64):
                request_id = f"full-tools-list-{page_index}"
                params = {} if cursor is None else {"cursor": cursor}
                send_json(
                    process,
                    {
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "method": "tools/list",
                        "params": params,
                    },
                )
                page_response, _ = read_response(lines, request_id, TIMEOUT_SECONDS)
                page_result = page_response.get("result", {})
                page = page_result.get("tools")
                if not isinstance(page, list) or not all(
                    isinstance(tool, dict) for tool in page
                ):
                    raise RuntimeError(
                        "packaged OpenBB MCP returned an invalid Tool page"
                    )
                inventory.extend(page)
                next_cursor = page_result.get("nextCursor")
                if next_cursor is None:
                    break
                if (
                    not isinstance(next_cursor, str)
                    or not next_cursor
                    or next_cursor in seen_cursors
                ):
                    raise RuntimeError(
                        "packaged OpenBB MCP returned an invalid Tool cursor"
                    )
                seen_cursors.add(next_cursor)
                cursor = next_cursor
            else:
                raise RuntimeError(
                    "packaged OpenBB MCP Tool pagination exceeded its limit"
                )

            discovered_provider_ids: set[str] = set()

            def collect_provider_ids(value: object) -> None:
                if isinstance(value, dict):
                    constant = value.get("const")
                    default = value.get("default")
                    enum = value.get("enum")
                    if isinstance(constant, str):
                        discovered_provider_ids.add(constant)
                    if isinstance(default, str):
                        discovered_provider_ids.add(default)
                    if isinstance(enum, list):
                        discovered_provider_ids.update(
                            item for item in enum if isinstance(item, str)
                        )
                    for nested in value.values():
                        collect_provider_ids(nested)
                elif isinstance(value, list):
                    for nested in value:
                        collect_provider_ids(nested)

            for tool in inventory:
                properties = tool.get("inputSchema", {}).get("properties", {})
                if isinstance(properties, dict):
                    collect_provider_ids(properties.get("provider", {}))
            if discovered_provider_ids != set(provider_ids):
                missing = sorted(set(provider_ids) - discovered_provider_ids)
                extra = sorted(discovered_provider_ids - set(provider_ids))
                raise RuntimeError(
                    "packaged OpenBB MCP provider discovery is incomplete: "
                    f"missing={missing}, extra={extra}"
                )
            if len(inventory) <= len(OPENBB_CONTROL_TOOLS):
                raise RuntimeError("packaged OpenBB MCP exposed no data Tools")
            stop_root_process(process)
            completed = True
        finally:
            finish_smoke_process(process, process_group, job, completed=completed)


COMPUTE_PROTOCOL = "guruterminal-compute/2"


def _compute_host_command(executable: Path, runtime_dir: Path, host: str) -> list[str]:
    return [
        str(executable),
        "run",
        "--no-config",
        "--no-lock",
        "--no-npm",
        "--node-modules-dir=none",
        "--cached-only",
        "--no-prompt",
        "--unstable-worker-options",
        "--v8-flags=--max-old-space-size=512",
        "--deny-import",
        "--deny-net",
        "--deny-env",
        "--deny-run",
        "--deny-write",
        "--deny-sys",
        "--deny-ffi",
        f"--allow-read={runtime_dir}",
        str(runtime_dir / host),
    ]


def _write_compute_line(
    process: subprocess.Popen[str], payload: dict[str, Any]
) -> None:
    if process.stdin is None:
        raise RuntimeError("compute worker stdin is missing")
    process.stdin.write(json.dumps(payload, separators=(",", ":")) + "\n")
    process.stdin.flush()


def _read_compute_line(lines: queue.Queue[str]) -> dict[str, Any]:
    try:
        line = lines.get(timeout=TIMEOUT_SECONDS)
    except queue.Empty as error:
        raise RuntimeError("compute worker response timed out") from error
    if not line:
        raise RuntimeError("compute worker closed without a response")
    value = json.loads(line)
    if not isinstance(value, dict):
        raise RuntimeError("compute worker response is not an object")
    return value


def compute_smoke(
    executable: Path,
    runtime_dir: Path,
    expected_deno_version: str,
    expected_pyodide_version: str,
) -> None:
    """Run packaged Python reuse and a capability-zero JavaScript cell."""
    runtime_dir = runtime_dir.resolve(strict=True)
    executable = executable.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="guruterminal-compute-smoke-") as name:
        environment = smoke_environment()
        environment.update(
            {
                "DENO_DIR": str(Path(name) / "deno-cache"),
                "DENO_NO_UPDATE_CHECK": "1",
                "NO_COLOR": "1",
            }
        )
        _python_compute_smoke(
            executable,
            runtime_dir,
            environment,
            expected_deno_version,
            expected_pyodide_version,
        )
        _javascript_compute_smoke(
            executable, runtime_dir, environment, expected_deno_version
        )


def _python_compute_smoke(
    executable: Path,
    runtime_dir: Path,
    environment: dict[str, str],
    expected_deno_version: str,
    expected_pyodide_version: str,
) -> None:
    process = subprocess.Popen(
        _compute_host_command(executable, runtime_dir, "bootstrap.mjs"),
        cwd=runtime_dir,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=environment,
        **spawn_options(),
    )
    process_group = process.pid
    job = own_process_tree(process, process_group)
    completed = False
    try:
        if process.stdout is None:
            raise RuntimeError("python compute worker stdout is missing")
        lines: queue.Queue[str] = queue.Queue()
        threading.Thread(
            target=stream_lines,
            args=(process.stdout, lines),
            daemon=True,
        ).start()
        _write_compute_line(
            process,
            {
                "protocol": COMPUTE_PROTOCOL,
                "type": "init",
                "language": "python",
                "packages": ["numpy", "pandas"],
            },
        )
        ready = _read_compute_line(lines)
        if ready.get("type") != "ready" or ready.get("language") != "python":
            raise RuntimeError("python compute host did not become ready")
        _write_compute_line(
            process,
            {
                "protocol": COMPUTE_PROTOCOL,
                "type": "run",
                "id": "00000000000000000000000000000001",
                "source": (
                    "def main(inputs):\n"
                    "    import js\n"
                    "    import numpy as np\n"
                    "    import pandas as pd\n"
                    "    values = np.asarray(inputs['values'], dtype=float)\n"
                    "    frame = pd.DataFrame({'value': values})\n"
                    "    return {'mean': frame['value'].mean(), "
                    "'rows': len(frame), 'deno_visible': hasattr(js, 'Deno')}\n"
                ),
                "inputs": {"values": [1, 2, 6]},
                "seed": 17,
            },
        )
        response = _read_compute_line(lines)
        runtime = response.get("runtime", {})
        data = response.get("result", {})
        if (
            response.get("protocol") != COMPUTE_PROTOCOL
            or response.get("type") != "result"
            or response.get("id") != "00000000000000000000000000000001"
            or response.get("ok") is not True
            or data.get("mean") != 3
            or data.get("rows") != 3
            or data.get("deno_visible") is not False
            or runtime.get("deno") != expected_deno_version
            or runtime.get("pyodide") != expected_pyodide_version
            or set(runtime.get("packages", {})) != {"numpy", "pandas"}
        ):
            raise RuntimeError("packaged compute result failed isolation checks")
        _write_compute_line(
            process,
            {"protocol": COMPUTE_PROTOCOL, "type": "shutdown"},
        )
        bye = _read_compute_line(lines)
        if bye.get("type") != "bye":
            raise RuntimeError("python compute host did not acknowledge shutdown")
        if process.stdin is not None:
            process.stdin.close()
        process.wait(timeout=TIMEOUT_SECONDS)
        stderr = process.stderr.read() if process.stderr is not None else ""
        if process.returncode != 0 or stderr:
            raise RuntimeError(f"compute worker failed: {stderr.strip()}")
        completed = True
    finally:
        finish_smoke_process(process, process_group, job, completed=completed)


def _javascript_compute_smoke(
    executable: Path,
    runtime_dir: Path,
    environment: dict[str, str],
    expected_deno_version: str,
) -> None:
    process = subprocess.Popen(
        _compute_host_command(executable, runtime_dir, "javascript-host.mjs"),
        cwd=runtime_dir,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=environment,
        **spawn_options(),
    )
    process_group = process.pid
    job = own_process_tree(process, process_group)
    completed = False
    try:
        if process.stdout is None:
            raise RuntimeError("javascript compute worker stdout is missing")
        lines: queue.Queue[str] = queue.Queue()
        threading.Thread(
            target=stream_lines,
            args=(process.stdout, lines),
            daemon=True,
        ).start()
        _write_compute_line(
            process,
            {
                "protocol": COMPUTE_PROTOCOL,
                "type": "init",
                "language": "javascript",
            },
        )
        ready = _read_compute_line(lines)
        if ready.get("type") != "ready" or ready.get("language") != "javascript":
            raise RuntimeError("javascript compute host did not become ready")
        _write_compute_line(
            process,
            {
                "protocol": COMPUTE_PROTOCOL,
                "type": "run",
                "id": "00000000000000000000000000000002",
                "source": (
                    "async function main(inputs) {\n"
                    "  return {\n"
                    "    value: inputs.value * 2,\n"
                    "    deno: typeof Deno,\n"
                    "    fetch: typeof fetch,\n"
                    "  };\n"
                    "}\n"
                ),
                "inputs": {"value": 3},
                "seed": 17,
            },
        )
        response = _read_compute_line(lines)
        runtime = response.get("runtime", {})
        data = response.get("result", {})
        if (
            response.get("protocol") != COMPUTE_PROTOCOL
            or response.get("type") != "result"
            or response.get("id") != "00000000000000000000000000000002"
            or response.get("ok") is not True
            or data.get("value") != 6
            or data.get("deno") != "undefined"
            or data.get("fetch") != "undefined"
            or runtime.get("language") != "javascript"
            or runtime.get("deno") != expected_deno_version
            or "pyodide" in runtime
        ):
            raise RuntimeError(
                "packaged javascript compute result failed isolation checks"
            )
        _write_compute_line(
            process,
            {"protocol": COMPUTE_PROTOCOL, "type": "shutdown"},
        )
        bye = _read_compute_line(lines)
        if bye.get("type") != "bye":
            raise RuntimeError("javascript compute host did not acknowledge shutdown")
        if process.stdin is not None:
            process.stdin.close()
        process.wait(timeout=TIMEOUT_SECONDS)
        stderr = process.stderr.read() if process.stderr is not None else ""
        if process.returncode != 0 or stderr:
            raise RuntimeError(f"javascript compute worker failed: {stderr.strip()}")
        completed = True
    finally:
        finish_smoke_process(process, process_group, job, completed=completed)


def provider_extension_smoke(
    executable: Path,
    runtime_dir: Path,
    extension: Path,
) -> None:
    """Prove the packaged Pi can resolve and run the provider extension in isolation."""
    if not extension.is_file():
        raise RuntimeError("provider extension is missing")
    run_controls = extension.with_name("model-run-controls.mjs")
    if not run_controls.is_file():
        raise RuntimeError("provider extension run-controls module is missing")
    native_search = extension.with_name("guruterminal-native-search.mjs")
    if not native_search.is_file():
        raise RuntimeError("provider extension native-search module is missing")
    native_search_modules = extension.with_name("native-search")
    if not native_search_modules.is_dir():
        raise RuntimeError("provider extension native-search modules are missing")
    with tempfile.TemporaryDirectory(prefix="guruterminal-provider-smoke-") as name:
        root = Path(name)
        if not WINDOWS:
            root.chmod(0o700)
        isolated_agent = root / "agent"
        isolated_agent.mkdir(mode=0o700)
        isolated_extension = isolated_agent / "guruterminal-provider-extension.mjs"
        shutil.copyfile(extension, isolated_extension)
        shutil.copyfile(run_controls, isolated_agent / run_controls.name)
        shutil.copyfile(native_search, isolated_agent / native_search.name)
        shutil.copytree(
            native_search_modules, isolated_agent / native_search_modules.name
        )
        result_path = root / "result.json"
        descriptor = os.open(result_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        os.close(descriptor)
        smoke_key = "guruterminal-packaged-provider-smoke"
        environment = pi_environment(runtime_dir)
        environment.update(
            {
                "PI_CODING_AGENT_DIR": str(root),
                "GEMINI_API_KEY": "guruterminal-provider-bootstrap",
                "GURUTERMINAL_PROVIDER_RESULT_FILE": str(result_path),
                "GURUTERMINAL_PROVIDER_API_KEY": smoke_key,
            }
        )
        process = subprocess.Popen(
            [
                str(executable),
                "--mode",
                "rpc",
                "--no-session",
                "--no-builtin-tools",
                "--no-extensions",
                "--extension",
                str(isolated_extension),
                "--no-skills",
                "--no-prompt-templates",
                "--no-themes",
                "--no-context-files",
                "--provider",
                "google",
                "--model",
                "gemini-2.5-flash",
            ],
            cwd=root,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            env=environment,
            **spawn_options(),
        )
        process_group = process.pid
        job = own_process_tree(process, process_group)
        completed = False
        try:
            if process.stdout is None:
                raise RuntimeError("packaged Pi stdout is unavailable")
            lines: queue.Queue[str] = queue.Queue()
            threading.Thread(
                target=stream_lines,
                args=(process.stdout, lines),
                daemon=True,
            ).start()
            send_json(
                process,
                {
                    "id": "credential-smoke",
                    "type": "prompt",
                    "message": "/guruterminal-provider-api-key anthropic set",
                },
            )
            deadline = time.monotonic() + TIMEOUT_SECONDS
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise RuntimeError("packaged Pi provider command timed out")
                message = read_json_line(lines, remaining)
                if (
                    message.get("type") == "response"
                    and message.get("id") == "credential-smoke"
                ):
                    if message.get("success") is not True:
                        raise RuntimeError("packaged Pi provider command failed")
                    break

            result = json.loads(result_path.read_text(encoding="utf-8"))
            if (
                result.get("protocol") != "guruterminal-provider/1"
                or result.get("type") != "credential_updated"
                or result.get("provider") != "anthropic"
                or result.get("models") != []
                or "credential" in result
            ):
                raise RuntimeError(
                    "packaged provider extension returned an invalid result"
                )
            auth_path = root / "auth.json"
            auth = json.loads(auth_path.read_text(encoding="utf-8"))
            if auth.get("anthropic", {}).get("key") != smoke_key:
                raise RuntimeError(
                    "Pi CredentialStore did not persist the smoke credential"
                )
            if not WINDOWS and auth_path.stat().st_mode & 0o777 != 0o600:
                raise RuntimeError(
                    "Pi CredentialStore produced an unsafe auth file mode"
                )
            stop_root_process(process)
            completed = True
        finally:
            finish_smoke_process(process, process_group, job, completed=completed)


def main() -> int:
    internal_arguments = sys.argv[1:]
    if internal_arguments == ["--windows-job-self-test"]:
        return windows_job_self_test()
    if internal_arguments == ["--windows-job-self-test-child"]:
        return windows_job_self_test_child()
    if (
        len(internal_arguments) == 2
        and internal_arguments[0] == "--windows-job-self-test-root"
    ):
        return windows_job_self_test_root(Path(internal_arguments[1]))

    parser = argparse.ArgumentParser()
    parser.add_argument("--pi", required=True, type=Path)
    parser.add_argument("--pi-version", required=True)
    parser.add_argument("--pi-runtime", required=True, type=Path)
    parser.add_argument("--provider-extension", required=True, type=Path)
    parser.add_argument("--core", required=True, type=Path)
    parser.add_argument("--core-version", required=True)
    parser.add_argument("--finance", required=True, type=Path)
    parser.add_argument("--compute", required=True, type=Path)
    parser.add_argument("--compute-runtime", required=True, type=Path)
    parser.add_argument("--deno-version", required=True)
    parser.add_argument("--pyodide-version", required=True)
    parser.add_argument("--openbb", required=True, type=Path)
    parser.add_argument("--openbb-runtime", required=True, type=Path)
    arguments = parser.parse_args()

    for executable in (
        arguments.pi,
        arguments.core,
        arguments.finance,
        arguments.compute,
        arguments.openbb,
    ):
        if not executable.is_file() or not os.access(executable, os.X_OK):
            raise SystemExit(f"missing executable: {executable}")

    environment = smoke_environment()
    one_shot_version(
        arguments.pi,
        arguments.pi_version,
        pi_environment(arguments.pi_runtime),
    )
    provider_extension_smoke(
        arguments.pi,
        arguments.pi_runtime,
        arguments.provider_extension,
    )
    one_shot_version(arguments.core, arguments.core_version, environment)
    worker_version, lock_digest = expected_finance_identity()
    finance_smoke(
        arguments.finance,
        finance_environment(),
        worker_version,
        lock_digest,
    )
    openbb_smoke(arguments.openbb, arguments.openbb_runtime)
    compute_smoke(
        arguments.compute,
        arguments.compute_runtime,
        arguments.deno_version,
        arguments.pyodide_version,
    )
    print("Sidecar smoke passed; no supervised launcher survived.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
