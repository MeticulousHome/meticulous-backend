"""The backend's API contracts against the snapshots in tests/contracts/.

Covers the HTTP routes and the methods each serves, the Socket.IO events the
server listens for and emits, the keys of the payloads clients read by name,
and the default settings. Any difference fails with a diff. When the change is
intended, regenerate the snapshots and commit them with it, so the contract
change is visible in review:

    UPDATE_CONTRACTS=1 uv run pytest tests/test_api_contracts.py
"""

import difflib
import json
import os
from pathlib import Path

import pytest

from tests.contracts import extract

CONTRACTS = Path(__file__).resolve().parent / "contracts"
UPDATE = os.getenv("UPDATE_CONTRACTS") == "1"


def _render(value) -> str:
    return json.dumps(value, indent=2, sort_keys=False) + "\n"


@pytest.mark.parametrize("snapshot", sorted(extract.SNAPSHOTS))
def test_contract_matches_its_snapshot(snapshot):
    current = _render(extract.SNAPSHOTS[snapshot]())
    path = CONTRACTS / snapshot
    if UPDATE:
        path.write_text(current)
        return

    assert path.exists(), f"{path} is missing; run with UPDATE_CONTRACTS=1 to create it"
    expected = path.read_text()
    if current != expected:
        diff = "".join(
            difflib.unified_diff(
                expected.splitlines(keepends=True),
                current.splitlines(keepends=True),
                fromfile=f"contracts/{snapshot} (snapshot)",
                tofile=f"contracts/{snapshot} (source)",
            )
        )
        pytest.fail(
            f"the backend's {snapshot} contract changed. If intended, run "
            f"UPDATE_CONTRACTS=1 uv run pytest tests/test_api_contracts.py and commit "
            f"the snapshot.\n{diff}"
        )


def test_no_route_is_registered_without_a_method():
    silent = [route["path"] for route in extract.http_routes() if not route["methods"]]
    assert silent == []
