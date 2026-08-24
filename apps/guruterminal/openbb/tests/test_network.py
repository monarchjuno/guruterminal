from __future__ import annotations

import subprocess
import sys

import pytest

from guruterminal_openbb.network import validate_https_url


def test_url_policy_requires_exact_https_origin() -> None:
    allowed = frozenset({"query1.finance.yahoo.com"})
    assert (
        validate_https_url("https://query1.finance.yahoo.com/path", allowed)
        == "https://query1.finance.yahoo.com/path"
    )
    for url in (
        "http://query1.finance.yahoo.com/path",
        "https://query2.finance.yahoo.com/path",
        "https://user:secret@query1.finance.yahoo.com/path",
        "https://query1.finance.yahoo.com:444/path",
    ):
        with pytest.raises(PermissionError):
            validate_https_url(url, allowed)


def test_runtime_guards_helpers_aliases_and_pinned_http_clients() -> None:
    script = r"""
import asyncio
import os
import sys
import types

from openbb_core.provider.utils import helpers
from guruterminal_openbb.network import apply_network_policy

alias = types.ModuleType("openbb_policy_test_alias")
alias.make_request = helpers.make_request
sys.modules[alias.__name__] = alias
previous = helpers.make_request
os.environ["HTTPS_PROXY"] = "https://proxy.invalid"
apply_network_policy({"query1.finance.yahoo.com"})
assert "HTTPS_PROXY" not in os.environ
assert alias.make_request is helpers.make_request
assert alias.make_request is not previous

def blocked(call, expected):
    try:
        call()
    except PermissionError as error:
        assert expected in str(error), str(error)
    else:
        raise AssertionError("request was not blocked")

blocked(
    lambda: helpers.make_request("https://api.benzinga.com/news"),
    "not enabled",
)

import requests
blocked(lambda: requests.Session().get("https://api.benzinga.com/news"), "not enabled")

import httpx
blocked(lambda: httpx.Client().get("https://api.benzinga.com/news"), "not enabled")

async def asgi_app(scope, receive, send):
    assert scope["type"] == "http"
    await send({"type": "http.response.start", "status": 200, "headers": []})
    await send({"type": "http.response.body", "body": b"ok"})

async def check_in_process_asgi():
    transport = httpx.ASGITransport(app=asgi_app)
    async with httpx.AsyncClient(
        transport=transport, base_url="http://guruterminal-in-process"
    ) as client:
        response = await client.get("/route")
        assert response.text == "ok"
asyncio.run(check_in_process_asgi())

import urllib.request
blocked(lambda: urllib.request.urlopen("http://query1.finance.yahoo.com"), "HTTPS only")

from curl_cffi import requests as curl_requests
blocked(
    lambda: curl_requests.Session().get("https://api.benzinga.com/news"),
    "not enabled",
)

import aiohttp
async def check_aiohttp():
    async with aiohttp.ClientSession() as session:
        try:
            await session.get("https://api.benzinga.com/news")
        except PermissionError as error:
            assert "not enabled" in str(error)
        else:
            raise AssertionError("aiohttp request was not blocked")
asyncio.run(check_aiohttp())
"""
    completed = subprocess.run(
        [sys.executable, "-c", script],
        capture_output=True,
        text=True,
        timeout=20,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
