import json

import pytest

from api import community_upload_diagnostics as diagnostics


@pytest.fixture
def snapshot(tmp_path, monkeypatch):
    monkeypatch.setenv("CONFIG_PATH", str(tmp_path))
    path = tmp_path / "community-upload" / "diagnostics.json"
    path.parent.mkdir()
    return path


def test_snapshot_allowlist(snapshot):
    snapshot.write_text(
        json.dumps(
            {
                "schemaVersion": 1,
                "capturedAt": 1,
                "connected": True,
                "pendingCount": 2,
                "privateSeed": "SECRET",
                "lastError": "SECRET",
                "recovery": {
                    "available": True,
                    "state": "running",
                    "added": 3,
                    "accessToken": "SECRET",
                },
                "lastFailure": {
                    "category": "upload_network",
                    "httpStatus": 503,
                    "requestId": "SECRET",
                    "body": "SECRET",
                },
            }
        )
    )
    result = diagnostics.collect()
    assert "SECRET" not in json.dumps(result)
    assert result["available"] and result["stale"]
    assert result["pendingCount"] == 2
    assert result["recovery"]["added"] == 3
    assert result["lastFailure"]["httpStatus"] == 503
    assert result["lastFailure"]["requestId"] is None


@pytest.mark.parametrize(
    "content,reason",
    [
        (None, "missing"),
        ("{", "unreadable"),
        ("x" * (diagnostics.MAX_BYTES + 1), "oversized"),
        ("[]", "unsupported_schema"),
    ],
)
def test_unavailable_snapshot(snapshot, content, reason):
    if content is not None:
        snapshot.write_text(content)
    result = diagnostics.collect()
    assert result["available"] is False
    assert result["reason"] == reason


def test_current_snapshot_and_correlation_id(snapshot):
    import time

    request_id = "08c2cbe7-112b-4145-baf3-e5d29df2e455"
    snapshot.write_text(
        json.dumps(
            {
                "schemaVersion": 1,
                "capturedAt": int(time.time()),
                "lastFailure": {
                    "category": "upload_failed",
                    "httpStatus": 401,
                    "requestId": request_id,
                },
            }
        )
    )
    result = diagnostics.collect()
    assert result["stale"] is False
    assert result["lastFailure"]["requestId"] == request_id
