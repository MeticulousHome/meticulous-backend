# Backend benchmark on the real firmware

`test_zz_benchmark.py` is a test for meticulous-integration's backend ×
firmware suite: it needs that repository's fixtures (fika-sil, the backend
started by `tests/boot/boot_harness.py --serve`, a Socket.IO client). It
measures, for whichever backend checkout the suite runs:

- the time from fika-sil turning the knob one detent to the client
  receiving the backend's `button` event (120 detents);
- the backend process's CPU and RSS over 30 s idle and over one shot of
  the default profile.

The firmware's share of the latency is the same for every backend, so two
runs compare backends. Run it in real time (`SIL_SPEED=1`):

```bash
cp docs/rust/benchmark/test_zz_benchmark.py ../meticulous-integration/tests/
cd ../meticulous-integration
SIL_SPEED=1 ci/local.sh ../meticulous-backend ../EspressoFirmware tests/test_zz_benchmark.py -s
```

It writes `artifacts/benchmark.json`. For the Python backend, point
`ci/local.sh` at a checkout of beta `f403168`. Remove the copied test from
meticulous-integration afterwards.
