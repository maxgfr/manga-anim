"""Calls the Node worker (workers/connect/run.mjs), which reaches providers through llm-connect.

One process per item: the request goes on stdin and the raw bytes come back in a file. The API
key stays in the environment, read by the worker itself; nothing here ever sees it.
Job fields: {"backend": "connect", "provider", "model", "script", "items": [...]}
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

from .openrouter import EXIT_REPORTED, Error


def generate(job: dict, kind: str, item: dict, out: Path, **extra) -> dict:
    """Runs one generation, writing the raw result to `out`. Returns the worker's JSON line."""
    request = {"kind": kind, "provider": job["provider"], "model": job["model"], "out": str(out), "item": item, **extra}
    try:
        done = subprocess.run(["node", job["script"]], input=json.dumps(request), capture_output=True, text=True)
    except FileNotFoundError:
        raise Error("Node.js not found: install it with `brew install node`") from None
    lines = [line for line in done.stderr.splitlines() if line.strip()]
    if done.returncode == EXIT_REPORTED:
        raise Error(lines[-1] if lines else "the connect worker reported an error with no detail")
    if done.returncode != 0:
        detail = lines[-1][:300] if lines else "no detail given"
        raise Error(f"the connect worker failed ({done.returncode}): {detail}")
    for line in lines:
        print(line, file=sys.stderr)
    return json.loads(done.stdout.strip().splitlines()[-1])
