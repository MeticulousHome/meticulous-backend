"""Contracts for the build-free inline pairing client."""

import ast
from pathlib import Path

PAIRING_SOURCE = Path(__file__).parents[1] / "api" / "pairing.py"


def _pair_page_html():
    tree = ast.parse(PAIRING_SOURCE.read_text())
    for node in tree.body:
        if not isinstance(node, ast.Assign):
            continue
        if any(
            isinstance(target, ast.Name) and target.id == "_PAIR_PAGE_HTML"
            for target in node.targets
        ):
            return ast.literal_eval(node.value)
    raise AssertionError("_PAIR_PAGE_HTML was not found")


def test_pair_page_fails_before_requesting_a_token_without_storage():
    html = _pair_page_html()
    initial_storage_gate = html.index("if (!storageIsWritable())")
    pair_request = html.index('fetch("/api/v1/pair/request"')

    assert initial_storage_gate < pair_request
    assert "This browser cannot save authorization." in html
    assert "localStorage.setItem(STORAGE_TEST_KEY" in html


def test_pair_page_validates_and_reports_credential_persistence():
    html = _pair_page_html()
    save_call = html.index("saveCredential(d);")
    success_ui = html.index('say("This device is now authorized.", "ok")', save_call)

    assert "function validCredential(value)" in html
    assert "The saved authorization was invalid." in html
    assert "The machine returned incomplete authorization." in html
    assert "This browser could not save authorization." in html
    assert save_call < success_ui
    assert "} catch (e) {}" not in html


def test_pair_page_serializes_attempts_and_uses_a_private_device_label():
    html = _pair_page_html()

    assert 'device_name: "Web browser"' in html
    assert "navigator.userAgent" not in html
    assert "pairingRevision" in html
    assert "if (verifying || !pairingId) return;" in html
    assert 'id="restartBtn"' in html
