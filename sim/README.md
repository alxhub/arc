# Virtual CAN lab

This lab runs the actual `psu-01` firmware core and the `dst-01` firmware crate
as host binaries, with one district node for each DST-01x4 revision. The nodes use the shared `link` codec to publish the
Presence and periodic or state-change district status frames in [PROTOCOL.md](../PROTOCOL.md),
receive district configuration frames, and run the same four district
controllers as the embedded build. A portable user-space bus carries CAN IDs and
CAN FD payload bytes between independent processes.

Start a bus and two nodes with Docker Compose:

```sh
docker compose -f sim/compose.yaml up --build
```

Node A builds with `host,rev1` and node B with `host,rev2`. PSU-01 closes the
simulated link switch after its monitor responds, qualifies 15 V, and starts a
repeating `FF 00 FF` DCC idle stream. The district nodes validate the packet
bits and each half-bit duration and require both power and recurring packets
before treating the source as ready. Every output remains disabled until
configured over CAN. Logs show identities, peer Presence, and state changes.
The host-only `--network-id` override supports collision
tests.

Run the first automated integration scenario:

```sh
python3 -m sim.scenarios short_recovery
python3 -m sim.scenarios district_config
python3 -m sim.scenarios psu_startup
python3 -m sim.scenarios psu_throttle
```

The runner starts an isolated Compose project, waits for both firmware nodes to
see a peer, configures districts 0 and 1 over CAN, then injects a short into each
revision's district 0,
checks that power is inhibited without disturbing district 1, clears the short,
and waits for normal operation to resume after ten clear probes. It reports the
last observed state and container logs on failure, then tears down its project.
It uses a test-only TCP control port on each host binary; these commands are
not ARC CAN messages and are absent from embedded firmware. The scenarios also
observe status frames on the virtual CAN bus, including windowed current after a
short.
The `district_config` scenario verifies default-disabled outputs, wrong-target
and invalid-state rejection, source-readiness gating, one-output enablement,
programming selection, and explicit disablement.
The `psu_startup` scenario verifies automatic link power, the exact idle packet
and timing, PSU CAN status, district source readiness on both revisions, and
loss of district output after a simulated PSU overvoltage latches the link off.
The `psu_throttle` scenario validates two locomotive entries, immediate CAN
status, idempotent repeat, stale-command rejection, and the DCC speed/stop
packet bytes. Both host and board builds accept valid throttle frames without
an authority exchange; that exchange remains future protocol work.

To exercise collision detection, start the optional third node with a different
UID and node A's ID:

```sh
docker compose -f sim/compose.yaml --profile collision up --build
```

Both nodes with ID `fa5138` report the conflict. Collision recovery is not
implemented.

The CAN transport uses newline-delimited JSON internally; this is not an ARC
wire format. It batches frames for 5 ms and orders each batch by CAN ID for
application tests. A separate signal bus broadcasts link power and DCC packets
as their bit sequences and half-bit durations, replaying the latest power
state to nodes that join late. Packet validity and liveness are modeled; analog
edge shape and scheduling jitter are not. CAN acknowledgements, errors, and
bus-off are also not modeled.

The host runner uses an initial board-component model. A DRV8874 model derives
fault and current observations from the applied drive and an injected short;
rev1 uses simulated TCA9534 output, direction, and fault-input registers, while
rev2 uses direct GPIO state. The current and fault behavior is intentionally
coarse. Embedded and host firmware share command decoding and status scheduling;
their HALs remain separate implementations, so hardware register behavior must be
checked against the board design as this model matures.

For fuller end-to-end validation, track and locomotive models will derive load
and occupancy from the PSU's DCC packet stream, and the DRV8874 will turn those
effects into sampled current and fault pins. A deterministic simulation clock and scripted fault injection will make
complete scenarios hermetic and repeatable without physical boards.

Run portable simulator tests with:

```sh
python3 -m unittest discover -s sim/tests
cd firmware && cargo test -p link && cargo test -p dst-01 --lib && cargo test -p psu-01 --lib
```
