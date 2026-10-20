# Offline energy session integration

`esp_serial.energy_session.EnergySession` owns an already-open serial link and
uses the firmware experiment's `energy-session-v1` contract. It is not imported
by `Machine`, registered as a service/endpoint, or enabled by a setting. There is
no device discovery, reset, firmware flashing, OS suspend or product timeout.

The owner serializes complete normal transactions, including multipart
hash/profile sends, with control exchanges. A transition closes admission before
waiting for the existing owner. A one-way `EnergyCommandWriter` lease is replaced
only after a matching RELEASED and a durable host completion record; refused
commands are never queued or replayed. Closing a writer proves software
ownership, not kernel/FIFO drain or firmware readiness.

Each normal request captures its writer and admission generation before
materializing multipart input or waiting for ownership. Both must still match
when it obtains the owner. A complete close/recover cycle cannot admit an older
request under the new writer. Admission changes and recovery publication share
a short, deadline-bounded lock that never surrounds caller iteration, owner
waiting, serial I/O or persistence.

A later transition revokes an earlier recovery's permission to publish, even
if the later request times out waiting for the owner. The earlier recovery may
finish the wire handshake and durable completion but raises `EnergyError` and
keeps commands closed. An explicit new prepare/recover cycle can then establish
a lease from that completed tombstone. Superseded transition waiters are rejected
before they can close a newer lease or perform control I/O. A command already
holding the owner may finish its whole transaction; its completion never reopens
admission.

## Durable sequence

An existing private directory and exclusive process ownership are required.
`IntentStore` writes a versioned record through a temporary file, file fsync,
atomic replace, directory fsync and readback. Corrupt/foreign records are not
cleared. Any uncertain save closes admission. This verifies OS calls and ordering;
it does not prove the target filesystem/media's power-loss behavior.

1. A fresh HELLO probe identifies the peer boot and its durable session. The host
   requires the fresh single-use device negotiation nonce for PREPARE and
   refuses unowned armed/released records. A completed host journal must match
   the peer session and sequence before another transaction may replace it.
   Missing or mismatched journals require diagnosis.
2. The host persists a new pending session before PREPARE. Matching READY proves
   only this experimental protocol's firmware barriers. It does not authorize
   Linux suspend or establish physical wake/power compliance. A matching WAKE
   cancels preparation immediately and retains pending intent for recovery.
3. Recovery obtains another fresh HELLO. It persists a recovery sequence higher
   than both records before sending RESUME with a fresh challenge. AWAKE carries
   the acquisition baseline. A matching SAMPLE must be newer and contain a finite
   pressure value from a new firmware acquisition, not a cached Socket.IO packet.
   This is one acquisition witness, not a complete sensor-health validation.
4. RELEASE binds the boot, session, sequence, challenge and accepted sample.
   Commands reopen only after matching RELEASED, durable host completion and a
   still-valid caller deadline, provided no later transition has revoked this
   recovery's generation. A new process starts closed even with a complete
   journal; it cannot automatically resume an old command lease.

The serial owner uses finite absolute deadlines and scopes both pyserial timeouts.
The Linux caller must use `CLOCK_BOOTTIME`. A short/interrupted/late write closes
ownership. Before each normal transaction, this offline candidate fsyncs a
`writing` record, then completes it only after a successful whole write. A crash
or uncertain partial normal frame therefore remains blocked across process
restart. External reset/reconnect reconciliation for that state is not implemented;
there is no method that erases the record to reopen. Per-command fsync cost and
media endurance need evaluation before any product integration.

A lost READY or RELEASED retains pending intent. Recovery uses a new challenge
and sequence; it does not infer success from silence. A peer reset requires its
new boot ID and reconciliation of the same durable intent. A persistence failure
may have committed either record, so the existing owner is poisoned rather than
trying to guess which bytes survived.

## Remaining integration gates

All command producers must actually move under this owner before activation,
including `Machine.write`, profile loading, API refusals, automatic settings sync,
reset and flashing. The current Machine runtime still uses its existing path.
The owner assumes exclusive process/port ownership and does not arbitrate a
second process or external serial tool. Observability needs an explicit energy
phase without fabricating valid-message timestamps.

OS wake registration, wakeup-count races, fallback RTC ownership, bounded wake
retries, physical UART capture, flash-wear/rollback policy, scale criteria and
measured power remain open. The firmware experiment is a separate build; neither
it nor the SDK UART diagnostic probe is a deployment artifact. No numerical
scale or power acceptance threshold is selected here.

The backend tests replace only the serial/OS boundary and use temporary files
and a test-created PTY. Actual firmware integration tests live in the private
firmware repository, outside this public repository. The existing package
`esp_serial/*.py` glob includes these unused modules.
