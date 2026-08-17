"""Release invariant for identity settlement before backend API readiness."""

from pathlib import Path


def test_identity_is_settled_and_signaled_before_http_listen():
    source = (Path(__file__).resolve().parents[1] / "backend.py").read_text(encoding="utf-8")

    remove_ready = source.index("os.remove(IDENTITY_READY_PATH)")
    recover_identifier = source.index("HostnameManager.init()")
    settle_identity = source.index("WifiManager.initializeIdentity()")
    publish_ready = source.index("os.replace(identity_ready_temporary, IDENTITY_READY_PATH)")
    listen = source.index('app.listen(PORT, address="127.0.0.1")')

    assert remove_ready < recover_identifier < settle_identity < publish_ready < listen
