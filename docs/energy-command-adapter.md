# Experimental command ownership

`esp_serial.energy_writer.EnergyCommandWriter` is an unused, one-way adapter for
the [coordinated-suspend proposal](https://github.com/MeticulousHome/meticulous-machine/blob/ed26e1f/research/energy_modes/protocol-design.md).
It accepts an already-open pyserial-compatible writer and an explicit clock.
Every operation requires a finite absolute deadline; a Linux integration must
use a clock including suspend. There is no selected product timeout, capability
advertisement, endpoint, setting, startup action or serial-device discovery.

Closing blocks future normal commands before waiting for an existing writer.
A failed barrier remains closed. Each write uses the remaining deadline as its
pyserial timeout; short writes and exceptions leave it closed because the peer
may have received an incomplete command. Refused commands are not queued. This
does not prove kernel/FIFO drain or firmware readiness, and deliberately avoids
unbounded `flush`/`tcdrain`. There is no recovery or release operation yet.

`Machine.write()` is not connected to this adapter. Integration must first move
all command producers under one owner, keep multipart hash/profile sends atomic,
propagate refusal to callers instead of reporting success, define bounded control
frames, persist/reconcile sessions and validate fresh post-resume telemetry.
Reset/flashing and external serial tools require separate ownership. A local
writer lock cannot prove the firmware is inhibited or authorize Linux suspend.

Tests exercise the actual adapter with a blocked writer, a short write, a timeout
and a real pyserial pseudo-terminal. The pseudo-terminal is created by the test;
no machine port or firmware simulator is opened. Existing package COPY rules
include `esp_serial/*.py`; this module has no runtime import or activation.
