"""Parse the one-shot Guru bootstrap frame before importing OpenBB.

Secrets arrive only in the first stdin line.  The remaining bytes on stdin are the
official MCP stdio transport and must not be buffered or replaced by this module.
"""

from __future__ import annotations

import json
import os
import re
import stat
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO, Final

BOOTSTRAP_TYPE: Final = "guruterminal.bootstrap"
BOOTSTRAP_PROTOCOL_VERSION: Final = 1
MAX_BOOTSTRAP_BYTES: Final = 65_536

_IDENTIFIER = re.compile(r"^[a-z][a-z0-9_]{0,63}$")
_RUN_ID = re.compile(r"^[A-Za-z0-9._:-]{1,128}$")
_DNS_HOST = re.compile(
    r"^(?=.{1,253}\Z)(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+"
    r"[a-z](?:[a-z0-9-]{0,61}[a-z0-9])?$"
)


class BootstrapError(ValueError):
    """Raised for an invalid or unsafe bootstrap frame."""


@dataclass(frozen=True, slots=True)
class BootstrapSettings:
    """Non-secret runtime restrictions supplied by the Rust authority."""

    allowed_categories: tuple[str, ...]
    enabled_provider_ids: tuple[str, ...]
    allowed_network_hosts: tuple[str, ...]
    provider_config: dict[str, dict[str, str]]


@dataclass(frozen=True, slots=True)
class Bootstrap:
    """Validated process bootstrap data."""

    run_id: str
    scratch_dir: Path
    credentials: dict[str, str]
    settings: BootstrapSettings


def _reject_duplicate_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise BootstrapError(f"duplicate bootstrap field: {key}")
        result[key] = value
    return result


def _read_string_list(value: object, field: str) -> tuple[str, ...]:
    if not isinstance(value, list):
        raise BootstrapError(f"bootstrap {field} must be an array")
    if len(value) > 128:
        raise BootstrapError(f"bootstrap {field} contains too many entries")

    items: list[str] = []
    seen: set[str] = set()
    for item in value:
        if not isinstance(item, str) or not _IDENTIFIER.fullmatch(item):
            raise BootstrapError(f"bootstrap {field} contains an invalid identifier")
        if item in seen:
            raise BootstrapError(f"bootstrap {field} contains a duplicate identifier")
        seen.add(item)
        items.append(item)
    return tuple(items)


def _read_network_hosts(value: object) -> tuple[str, ...]:
    if not isinstance(value, list):
        raise BootstrapError("bootstrap allowed_network_hosts must be an array")
    if len(value) > 256:
        raise BootstrapError(
            "bootstrap allowed_network_hosts contains too many entries"
        )

    items: list[str] = []
    seen: set[str] = set()
    for item in value:
        if not isinstance(item, str) or not _DNS_HOST.fullmatch(item):
            raise BootstrapError(
                "bootstrap allowed_network_hosts contains an invalid exact DNS hostname"
            )
        if item in seen:
            raise BootstrapError(
                "bootstrap allowed_network_hosts contains a duplicate hostname"
            )
        seen.add(item)
        items.append(item)
    return tuple(items)


def _validate_scratch_dir(value: object) -> Path:
    if not isinstance(value, str) or not value:
        raise BootstrapError("bootstrap scratch_dir must be a non-empty string")
    supplied = Path(value)
    if not supplied.is_absolute():
        raise BootstrapError("bootstrap scratch_dir must be absolute")
    try:
        resolved = supplied.resolve(strict=True)
    except OSError as error:
        raise BootstrapError("bootstrap scratch_dir does not exist") from error
    if supplied.is_symlink() or not resolved.is_dir():
        raise BootstrapError("bootstrap scratch_dir must be a real directory")
    if os.name != "nt":
        mode = stat.S_IMODE(resolved.stat().st_mode)
        if mode & (stat.S_IRWXG | stat.S_IRWXO):
            raise BootstrapError("bootstrap scratch_dir must be private (mode 0700)")
    return resolved


def _parse_credentials(value: object) -> dict[str, str]:
    if not isinstance(value, dict):
        raise BootstrapError("bootstrap credentials must be an object")
    if len(value) > 64:
        raise BootstrapError("bootstrap credentials contains too many entries")

    credentials: dict[str, str] = {}
    for key, secret in value.items():
        if not isinstance(key, str) or not _IDENTIFIER.fullmatch(key):
            raise BootstrapError("bootstrap credentials contains an invalid key")
        if not isinstance(secret, str) or not secret or len(secret) > 16_384:
            raise BootstrapError(f"bootstrap credential {key} is invalid")
        credentials[key] = secret
    return credentials


def _parse_provider_config(value: object) -> dict[str, dict[str, str]]:
    if not isinstance(value, dict):
        raise BootstrapError("bootstrap provider_config must be an object")
    if len(value) > 32:
        raise BootstrapError("bootstrap provider_config contains too many providers")

    result: dict[str, dict[str, str]] = {}
    for provider_id, provider_values in value.items():
        if not isinstance(provider_id, str) or not _IDENTIFIER.fullmatch(provider_id):
            raise BootstrapError("bootstrap provider_config has an invalid provider")
        if not isinstance(provider_values, dict) or len(provider_values) > 16:
            raise BootstrapError(
                f"bootstrap provider_config for {provider_id} must be a bounded object"
            )
        parsed: dict[str, str] = {}
        for key, config_value in provider_values.items():
            if not isinstance(key, str) or not _IDENTIFIER.fullmatch(key):
                raise BootstrapError(
                    f"bootstrap provider_config for {provider_id} has an invalid key"
                )
            if (
                not isinstance(config_value, str)
                or not config_value
                or len(config_value) > 512
                or any(
                    ord(character) < 32 or ord(character) == 127
                    for character in config_value
                )
            ):
                raise BootstrapError(
                    f"bootstrap provider_config value for {provider_id}.{key} is invalid"
                )
            parsed[key] = config_value
        result[provider_id] = parsed
    return result


def parse_bootstrap(payload: object) -> Bootstrap:
    """Validate a decoded bootstrap object without importing OpenBB."""

    if not isinstance(payload, dict):
        raise BootstrapError("bootstrap frame must be a JSON object")
    expected = {
        "type",
        "protocol_version",
        "run_id",
        "scratch_dir",
        "credentials",
        "settings",
    }
    unknown = set(payload) - expected
    missing = expected - set(payload)
    if unknown:
        raise BootstrapError(
            f"bootstrap frame contains unknown fields: {', '.join(sorted(unknown))}"
        )
    if missing:
        raise BootstrapError(
            f"bootstrap frame is missing fields: {', '.join(sorted(missing))}"
        )
    if payload["type"] != BOOTSTRAP_TYPE:
        raise BootstrapError("bootstrap frame has the wrong type")
    if payload["protocol_version"] != BOOTSTRAP_PROTOCOL_VERSION:
        raise BootstrapError("unsupported bootstrap protocol version")

    run_id = payload["run_id"]
    if not isinstance(run_id, str) or not _RUN_ID.fullmatch(run_id):
        raise BootstrapError("bootstrap run_id is invalid")

    settings = payload["settings"]
    if not isinstance(settings, dict):
        raise BootstrapError("bootstrap settings must be an object")
    expected_settings = {
        "allowed_categories",
        "enabled_provider_ids",
        "allowed_network_hosts",
        "provider_config",
    }
    if set(settings) != expected_settings:
        raise BootstrapError(
            "bootstrap settings must contain only allowed_categories, "
            "enabled_provider_ids, allowed_network_hosts, and provider_config"
        )

    return Bootstrap(
        run_id=run_id,
        scratch_dir=_validate_scratch_dir(payload["scratch_dir"]),
        credentials=_parse_credentials(payload["credentials"]),
        settings=BootstrapSettings(
            allowed_categories=_read_string_list(
                settings["allowed_categories"], "allowed_categories"
            ),
            enabled_provider_ids=_read_string_list(
                settings["enabled_provider_ids"], "enabled_provider_ids"
            ),
            allowed_network_hosts=_read_network_hosts(
                settings["allowed_network_hosts"]
            ),
            provider_config=_parse_provider_config(settings["provider_config"]),
        ),
    )


def read_bootstrap(stream: BinaryIO) -> Bootstrap:
    """Consume exactly one newline-terminated bootstrap frame from ``stream``."""

    raw = bytearray(stream.readline(MAX_BOOTSTRAP_BYTES + 1))
    try:
        if not raw:
            raise BootstrapError("missing bootstrap frame")
        if len(raw) > MAX_BOOTSTRAP_BYTES:
            raise BootstrapError("bootstrap frame is too large")
        if raw[-1:] != b"\n":
            raise BootstrapError("bootstrap frame must end with a newline")
        try:
            text = raw.decode("utf-8")
        except UnicodeDecodeError as error:
            raise BootstrapError("bootstrap frame is not UTF-8") from error
        try:
            payload = json.loads(text, object_pairs_hook=_reject_duplicate_keys)
        except json.JSONDecodeError as error:
            raise BootstrapError("bootstrap frame is not valid JSON") from error
        return parse_bootstrap(payload)
    finally:
        for index in range(len(raw)):
            raw[index] = 0


def configure_scratch_environment(scratch_dir: Path) -> None:
    """Point OpenBB and common dependency state at the private run directory."""

    directories = {
        "home": scratch_dir / "home",
        "config": scratch_dir / "config",
        "cache": scratch_dir / "cache",
        "data": scratch_dir / "data",
        "tmp": scratch_dir / "tmp",
        "matplotlib": scratch_dir / "matplotlib",
        "numba": scratch_dir / "numba",
        "pycache": scratch_dir / "pycache",
    }
    for directory in directories.values():
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)

    home = str(directories["home"])
    os.environ.update(
        {
            "HOME": home,
            "USERPROFILE": home,
            "XDG_CONFIG_HOME": str(directories["config"]),
            "XDG_CACHE_HOME": str(directories["cache"]),
            "XDG_DATA_HOME": str(directories["data"]),
            "TMPDIR": str(directories["tmp"]),
            "TEMP": str(directories["tmp"]),
            "TMP": str(directories["tmp"]),
            "MPLCONFIGDIR": str(directories["matplotlib"]),
            "NUMBA_CACHE_DIR": str(directories["numba"]),
            "PYTHONPYCACHEPREFIX": str(directories["pycache"]),
            "OPENBB_AUTO_BUILD": "false",
            "OPENBB_DEBUG_MODE": "false",
            "OPENBB_DEV_MODE": "false",
            "FASTMCP_CHECK_FOR_UPDATES": "off",
            "FASTMCP_SHOW_SERVER_BANNER": "false",
        }
    )
    tempfile.tempdir = str(directories["tmp"])
