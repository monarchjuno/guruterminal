from __future__ import annotations

import subprocess
import sys
from pathlib import Path


def test_credentials_feed_rest_context_without_persisting(tmp_path: Path) -> None:
    tmp_path.chmod(0o700)
    secret = "guru-secret-sentinel-2ae874"
    script = """
import os
import sys
from pathlib import Path
from guruterminal_openbb.bootstrap import configure_scratch_environment

scratch = Path(sys.argv[1])
secret = sys.stdin.read()
configure_scratch_environment(scratch)
from guruterminal_openbb.server import apply_credentials, apply_sec_contact_email
apply_credentials({"tradier_api_key": secret, "tradier_account_type": "sandbox"})

from openbb_core.app.service.user_service import UserService
value = UserService.read_from_file().credentials.tradier_api_key
assert value is not None
assert value.get_secret_value() == secret
account_type = UserService.read_from_file().credentials.tradier_account_type
assert account_type is not None
assert account_type.get_secret_value() == "sandbox"

import asyncio
from openbb_core.provider.utils import helpers as provider_helpers
from openbb_core.provider.utils.errors import EmptyDataError
from openbb_sec.models.latest_financial_reports import (
    SecLatestFinancialReportsFetcher,
    SecLatestFinancialReportsQueryParams,
)
from openbb_sec.utils import definitions, form4, helpers as sec_helpers

async_calls = []
sync_calls = []

async def fake_async_request(url, *args, **kwargs):
    async_calls.append((url, kwargs.get("headers")))
    return {"hits": {"total": {"value": 0}, "hits": []}}

def fake_sync_request(url, *args, **kwargs):
    sync_calls.append((url, kwargs.get("headers")))
    return object()

# Import the SEC helper first so the patch must also replace an already-bound
# module alias. The latest-financial-reports route imports the core helper from
# inside aextract_data and constructs its own placeholder SEARCH_HEADERS.
provider_helpers.amake_request = fake_async_request
provider_helpers.make_request = fake_sync_request
sec_helpers.amake_request = fake_async_request
sec_helpers.make_request = fake_sync_request
apply_sec_contact_email("research@example.com")

expected_identity = "Guru Terminal research@example.com"
assert definitions.HEADERS["User-Agent"] == "Guru Terminal research@example.com"
assert definitions.SEC_HEADERS["User-Agent"] == "Guru Terminal research@example.com"
assert form4.SEC_HEADERS["User-Agent"] == expected_identity

try:
    asyncio.run(
        SecLatestFinancialReportsFetcher.aextract_data(
            SecLatestFinancialReportsQueryParams(
                date="2024-01-02", report_type="10-K"
            ),
            None,
        )
    )
except EmptyDataError:
    pass
else:
    raise AssertionError("empty fake SEC result should raise EmptyDataError")

asyncio.run(
    sec_helpers.amake_request(
        "https://www.sec.gov/files/company_tickers.json",
        headers={"user-agent": "another placeholder"},
    )
)
provider_helpers.make_request(
    "https://www.sec.gov/info/edgar/edgartaxonomies.xml"
)

assert len(async_calls) == 2
assert all(headers["User-Agent"] == expected_identity for _, headers in async_calls)
assert all(
    all(str(key).lower() != "user-agent" or value == expected_identity
        for key, value in headers.items())
    for _, headers in async_calls
)
assert sync_calls[0][1]["User-Agent"] == expected_identity

placeholder_fragments = ("fakecompany", "Jesus Window Washing", "stainedglass.com")
for module in (definitions, form4):
    for value in vars(module).values():
        if isinstance(value, dict):
            rendered = repr(value)
            assert not any(fragment in rendered for fragment in placeholder_fragments)

needle = secret.encode()
for candidate in scratch.rglob("*"):
    if candidate.is_file():
        assert needle not in candidate.read_bytes(), candidate
"""
    completed = subprocess.run(
        [sys.executable, "-c", script, str(tmp_path)],
        check=False,
        capture_output=True,
        text=True,
        input=secret,
        timeout=30,
    )
    assert completed.returncode == 0, completed.stderr
