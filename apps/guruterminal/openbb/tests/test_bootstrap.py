from __future__ import annotations

import io
import json
import os
import tempfile
from pathlib import Path

import pytest

from guruterminal_openbb.bootstrap import (
    MAX_BOOTSTRAP_BYTES,
    BootstrapError,
    configure_scratch_environment,
    read_bootstrap,
)
from guruterminal_openbb.manifest import (
    authorize_bootstrap,
    load_runtime_manifest,
    resolve_network_hosts,
)


def bootstrap_frame(
    scratch: Path,
    *,
    credentials: dict[str, str] | None = None,
    providers: list[str] | None = None,
) -> bytes:
    selected_providers = providers or ["yfinance"]
    value = {
        "type": "guruterminal.bootstrap",
        "protocol_version": 1,
        "run_id": "run-123",
        "scratch_dir": str(scratch),
        "credentials": credentials or {},
        "settings": {
            "allowed_categories": ["equity"],
            "enabled_provider_ids": selected_providers,
            "allowed_network_hosts": sorted(
                resolve_network_hosts(set(selected_providers), load_runtime_manifest())
            ),
            "provider_config": {},
        },
    }
    return (json.dumps(value) + "\n").encode()


def test_bootstrap_consumes_only_the_first_line(tmp_path: Path) -> None:
    tmp_path.chmod(0o700)
    mcp_frame = b'{"jsonrpc":"2.0","method":"initialize","id":1}\n'
    stream = io.BytesIO(bootstrap_frame(tmp_path) + mcp_frame)

    bootstrap = read_bootstrap(stream)

    assert bootstrap.run_id == "run-123"
    assert bootstrap.settings.allowed_categories == ("equity",)
    assert stream.readline() == mcp_frame


@pytest.mark.parametrize(
    "frame, message",
    [
        (b"", "missing bootstrap frame"),
        (b"{}", "must end with a newline"),
        (b"not-json\n", "not valid JSON"),
        (
            b'{"type":"guruterminal.bootstrap","type":"duplicate"}\n',
            "duplicate bootstrap field",
        ),
        (b"x" * (MAX_BOOTSTRAP_BYTES + 1), "too large"),
    ],
)
def test_bootstrap_rejects_invalid_frames(frame: bytes, message: str) -> None:
    with pytest.raises(BootstrapError, match=message):
        read_bootstrap(io.BytesIO(frame))


def test_bootstrap_requires_private_real_scratch_directory(tmp_path: Path) -> None:
    if os.name == "nt":
        pytest.skip("POSIX permission assertion")
    tmp_path.chmod(0o755)

    with pytest.raises(BootstrapError, match="private"):
        read_bootstrap(io.BytesIO(bootstrap_frame(tmp_path)))


def test_manifest_authorizes_only_enabled_provider_credentials(tmp_path: Path) -> None:
    tmp_path.chmod(0o700)
    bootstrap = read_bootstrap(
        io.BytesIO(
            bootstrap_frame(
                tmp_path,
                credentials={"fmp_api_key": "secret"},
                providers=["fmp"],
            )
        )
    )
    assert authorize_bootstrap(bootstrap, load_runtime_manifest()) is bootstrap

    denied = read_bootstrap(
        io.BytesIO(
            bootstrap_frame(
                tmp_path,
                credentials={"fmp_api_key": "secret"},
                providers=["yfinance"],
            )
        )
    )
    with pytest.raises(BootstrapError, match="disabled provider"):
        authorize_bootstrap(denied, load_runtime_manifest())


def test_manifest_requires_exact_enabled_provider_host_union(tmp_path: Path) -> None:
    tmp_path.chmod(0o700)
    value = json.loads(bootstrap_frame(tmp_path, providers=["yfinance"]))
    value["settings"]["allowed_network_hosts"].append("api.benzinga.com")
    bootstrap = read_bootstrap(io.BytesIO((json.dumps(value) + "\n").encode()))

    with pytest.raises(BootstrapError, match="exactly match"):
        authorize_bootstrap(bootstrap, load_runtime_manifest())


@pytest.mark.parametrize(
    "host",
    ["*", "HTTPS://query1.finance.yahoo.com", "localhost", "query1.finance.yahoo.com."],
)
def test_bootstrap_rejects_non_exact_dns_hosts(tmp_path: Path, host: str) -> None:
    tmp_path.chmod(0o700)
    value = json.loads(bootstrap_frame(tmp_path))
    value["settings"]["allowed_network_hosts"] = [host]

    with pytest.raises(BootstrapError, match="exact DNS hostname"):
        read_bootstrap(io.BytesIO((json.dumps(value) + "\n").encode()))


def test_manifest_authorizes_only_declared_provider_config(tmp_path: Path) -> None:
    tmp_path.chmod(0o700)
    value = json.loads(bootstrap_frame(tmp_path))
    value["settings"]["enabled_provider_ids"] = ["sec"]
    value["settings"]["allowed_network_hosts"] = sorted(
        resolve_network_hosts({"sec"}, load_runtime_manifest())
    )
    value["settings"]["provider_config"] = {
        "sec": {"contact_email": "research@example.com"}
    }
    bootstrap = read_bootstrap(io.BytesIO((json.dumps(value) + "\n").encode()))
    assert authorize_bootstrap(bootstrap, load_runtime_manifest()) is bootstrap

    value["settings"]["provider_config"] = {"sec": {"unknown": "value"}}
    denied = read_bootstrap(io.BytesIO((json.dumps(value) + "\n").encode()))
    with pytest.raises(BootstrapError, match="unknown provider config"):
        authorize_bootstrap(denied, load_runtime_manifest())


def test_scratch_environment_redirects_home_and_caches(tmp_path: Path) -> None:
    tmp_path.chmod(0o700)
    keys = {
        "HOME",
        "USERPROFILE",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_DATA_HOME",
        "TMPDIR",
        "TEMP",
        "TMP",
        "MPLCONFIGDIR",
        "NUMBA_CACHE_DIR",
        "PYTHONPYCACHEPREFIX",
        "OPENBB_AUTO_BUILD",
        "OPENBB_DEBUG_MODE",
        "OPENBB_DEV_MODE",
        "FASTMCP_CHECK_FOR_UPDATES",
        "FASTMCP_SHOW_SERVER_BANNER",
    }
    previous = {key: os.environ.get(key) for key in keys}
    previous_tempdir = tempfile.tempdir
    try:
        configure_scratch_environment(tmp_path)
        for key in keys - {
            "OPENBB_AUTO_BUILD",
            "OPENBB_DEBUG_MODE",
            "OPENBB_DEV_MODE",
            "FASTMCP_CHECK_FOR_UPDATES",
            "FASTMCP_SHOW_SERVER_BANNER",
        }:
            assert Path(os.environ[key]).is_relative_to(tmp_path)
        assert os.environ["OPENBB_AUTO_BUILD"] == "false"
        assert os.environ["FASTMCP_CHECK_FOR_UPDATES"] == "off"
        assert os.environ["FASTMCP_SHOW_SERVER_BANNER"] == "false"
        assert tempfile.tempdir == str(tmp_path / "tmp")
    finally:
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        tempfile.tempdir = previous_tempdir
