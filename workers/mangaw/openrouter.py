"""Small OpenRouter HTTP client shared by the workers (stdlib only).

The API key is read from OPENROUTER_API_KEY and only ever travels in the Authorization header
to the OpenRouter origin: it is never logged, written to a job file or sent across a redirect.
"""

from __future__ import annotations

import base64
import contextlib
import json
import mimetypes
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Iterator

DEFAULT_BASE_URL = "https://openrouter.ai/api/v1"
KEY_ENV = "OPENROUTER_API_KEY"
# Used only by the tests, and restricted to loopback so it can never send the key elsewhere.
BASE_URL_ENV = "MANGA_OPENROUTER_BASE_URL"
LOOPBACK = {"127.0.0.1", "localhost", "::1"}

# The Rust side treats this exit code as "the worker already printed the error".
EXIT_REPORTED = 3

REQUEST_TIMEOUT_S = 300
RETRIES = 3
RETRY_DELAY_S = 1.0
# Rejected before generation starts, so a retry cannot bill twice.
RETRYABLE = {429, 502, 503}


class Error(Exception):
    """A failure with a one-line, actionable message."""


class Transient(Error):
    """A temporary failure: retrying later may work."""


@contextlib.contextmanager
def reported() -> Iterator[None]:
    """Prints an Error on one line and exits with the code Rust recognizes."""
    try:
        yield
    except Error as e:
        print(e, file=sys.stderr)
        sys.exit(EXIT_REPORTED)


def base_url() -> str:
    override = os.environ.get(BASE_URL_ENV, "").strip()
    if not override:
        return DEFAULT_BASE_URL
    if urllib.parse.urlsplit(override).hostname not in LOOPBACK:
        raise Error(f"{BASE_URL_ENV} is for tests only and must point to a loopback address")
    return override.rstrip("/")


def origin(url: str) -> tuple[str, str | None, int | None]:
    parts = urllib.parse.urlsplit(url)
    return parts.scheme, parts.hostname, parts.port


class SameOriginAuth(urllib.request.HTTPRedirectHandler):
    """urllib forwards every header on a redirect: drop the key when leaving the origin."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        new = super().redirect_request(req, fp, code, msg, headers, newurl)
        if new is not None and origin(newurl) != origin(req.full_url):
            new.remove_header("Authorization")
        return new


def data_url(path: str) -> str:
    """A local image as a base64 data URL, the form OpenRouter accepts for local files."""
    mime = mimetypes.guess_type(path, strict=False)[0] or "image/png"
    with open(path, "rb") as f:
        return f"data:{mime};base64,{base64.standard_b64encode(f.read()).decode()}"


def failure_message(error) -> str:
    """The text of an `error` field: a string, or an object with a `message`."""
    if isinstance(error, dict):
        error = error.get("message") or json.dumps(error)
    return str(error) if error else ""


class Client:
    def __init__(self) -> None:
        self.key = os.environ.get(KEY_ENV, "").strip()
        if not self.key:
            raise Error(f"{KEY_ENV} is not set: create a key at https://openrouter.ai/settings/keys and export it")
        self.base = base_url()
        self.opener = urllib.request.build_opener(SameOriginAuth)

    def one_line(self, text: str, limit: int = 400) -> str:
        return " ".join(str(text).replace(self.key, "***").split())[:limit]

    def http_error(self, status: int, raw: bytes) -> Error:
        if status == 401:
            return Error(f"OpenRouter rejected the API key (401): set a valid {KEY_ENV} (https://openrouter.ai/settings/keys)")
        if status == 402:
            return Error("OpenRouter has no credits left on this account (402): add some at https://openrouter.ai/settings/credits, then rerun (finished items stay cached)")
        try:
            detail = failure_message(json.loads(raw).get("error"))
        except (ValueError, AttributeError):
            detail = raw.decode("utf-8", "replace")
        detail = self.one_line(detail) or "no detail given"
        kind = Transient if status in RETRYABLE or status >= 500 else Error
        return kind(f"OpenRouter request failed ({status}): {detail}")

    def call(self, method: str, path: str, body: dict | None = None) -> tuple[bytes, str]:
        """Returns (body, content type), retrying what cannot have been billed."""
        data = None if body is None else json.dumps(body).encode()
        headers = {"Authorization": f"Bearer {self.key}"}
        if data is not None:
            headers["Content-Type"] = "application/json"
        failure: Error = Transient("OpenRouter request failed")
        for attempt in range(RETRIES + 1):
            if attempt:
                time.sleep(delay)
            delay = RETRY_DELAY_S * 2**attempt
            request = urllib.request.Request(self.base + path, data=data, method=method, headers=headers)
            try:
                with self.opener.open(request, timeout=REQUEST_TIMEOUT_S) as response:
                    return response.read(), response.headers.get_content_type()
            except urllib.error.HTTPError as e:
                failure = self.http_error(e.code, e.read())
                if e.code not in RETRYABLE and not (method == "GET" and e.code >= 500):
                    raise failure from None
                if (after := e.headers.get("Retry-After", "")).isdigit():
                    delay = min(int(after), 60)
            except (urllib.error.URLError, OSError) as e:
                failure = Transient(f"OpenRouter is unreachable: {self.one_line(getattr(e, 'reason', e))}")
                # A POST may have reached the server: resending it could bill twice.
                if method != "GET":
                    raise failure from None
        raise failure

    def parse(self, raw: bytes) -> dict:
        try:
            parsed = json.loads(raw)
        except ValueError:
            raise Error("OpenRouter returned a response that is not JSON") from None
        if not isinstance(parsed, dict):
            raise Error("OpenRouter returned an unexpected JSON response")
        return parsed

    def checked(self, raw: bytes) -> dict:
        """Parsed JSON, failing on an `error` field: failures can come with a 200 status."""
        parsed = self.parse(raw)
        if parsed.get("error"):
            raise Error(f"OpenRouter request failed: {self.one_line(failure_message(parsed['error']))}")
        return parsed

    def get_json(self, path: str) -> dict:
        return self.checked(self.call("GET", path)[0])

    def post_json(self, path: str, body: dict) -> dict:
        return self.checked(self.call("POST", path, body)[0])

    def post_binary(self, path: str, body: dict) -> tuple[bytes, str]:
        raw, content_type = self.call("POST", path, body)
        if content_type == "application/json":
            self.checked(raw)
            raise Error("OpenRouter returned JSON instead of the expected binary data")
        return raw, content_type

    def model_entry(self, listing: str, model: str, kind: str) -> dict:
        """The capabilities of `model` in a listing such as /videos/models. Free of charge."""
        for entry in self.get_json(listing).get("data", []):
            if model in (entry.get("id"), entry.get("canonical_slug")):
                return entry
        raise Error(f"unknown OpenRouter {kind} model \"{model}\": pick an id from https://openrouter.ai/models?output_modalities={kind}")
