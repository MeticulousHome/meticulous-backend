from ble_auth import (
    AUTH_WINDOW_SECONDS,
    PROMPT_COOLDOWN_SECONDS,
    BleAuthorization,
)

T0 = 1_000_000.0


def test_starts_unauthorized():
    auth = BleAuthorization()
    assert not auth.active(T0)
    assert auth.remaining(T0) == 0.0


def test_grant_opens_window_from_ack_timestamp():
    auth = BleAuthorization()
    auth.grant(T0)
    assert auth.active(T0)
    assert auth.active(T0 + AUTH_WINDOW_SECONDS - 1)
    assert not auth.active(T0 + AUTH_WINDOW_SECONDS)


def test_grant_is_idempotent():
    # The notification layer can deliver the acknowledgement callback twice.
    auth = BleAuthorization()
    auth.grant(T0)
    auth.grant(T0)
    assert auth.active(T0 + 1)


def test_revoke_closes_window():
    auth = BleAuthorization()
    auth.grant(T0)
    auth.revoke()
    assert not auth.active(T0 + 1)


def test_expiry_forgets_grant():
    auth = BleAuthorization()
    auth.grant(T0)
    assert not auth.active(T0 + AUTH_WINDOW_SECONDS + 1)
    # A later check well inside what would have been a fresh window is still
    # unauthorized: expiry cleared the grant instead of leaving a stale one.
    assert not auth.active(T0 + AUTH_WINDOW_SECONDS + 2)


def test_remaining_counts_down():
    auth = BleAuthorization()
    auth.grant(T0)
    assert auth.remaining(T0) == AUTH_WINDOW_SECONDS
    assert auth.remaining(T0 + 30) == AUTH_WINDOW_SECONDS - 30
    assert auth.remaining(T0 + AUTH_WINDOW_SECONDS + 5) == 0.0


def test_should_prompt_initially():
    auth = BleAuthorization()
    assert auth.should_prompt(prompt_pending=False, now=T0)


def test_no_prompt_while_one_is_pending():
    auth = BleAuthorization()
    assert not auth.should_prompt(prompt_pending=True, now=T0)


def test_no_prompt_while_authorized():
    auth = BleAuthorization()
    auth.grant(T0)
    assert not auth.should_prompt(prompt_pending=False, now=T0 + 10)


def test_prompt_cooldown_blunts_spam():
    # A hostile peer re-reading the status characteristic after a denial must
    # not make the Dial chime continuously.
    auth = BleAuthorization()
    auth.note_prompt(T0)
    auth.revoke()
    assert not auth.should_prompt(prompt_pending=False, now=T0 + 1)
    assert not auth.should_prompt(prompt_pending=False, now=T0 + PROMPT_COOLDOWN_SECONDS - 1)
    assert auth.should_prompt(prompt_pending=False, now=T0 + PROMPT_COOLDOWN_SECONDS)


def test_reprompt_after_window_expires():
    auth = BleAuthorization()
    auth.note_prompt(T0)
    auth.grant(T0 + 5)
    expired = T0 + 5 + AUTH_WINDOW_SECONDS + 1
    assert auth.should_prompt(prompt_pending=False, now=expired)
