# Virtual CAN lab

This lab runs the actual `dst-01` firmware crate as host binaries, built once
for each DST-01x4 revision. The nodes use the shared `link` codec to publish the
Presence frame in [PROTOCOL.md](../PROTOCOL.md), and run the same four district
controllers as the embedded build. A portable user-space bus carries CAN IDs and
CAN FD payload bytes between independent processes.

Start a bus and two nodes with Docker Compose:

```sh
docker compose -f sim/compose.yaml up --build
```

Node A builds with `host,rev1` and node B with `host,rev2`. Their logs show
identities, peer Presence, and district state changes. Both start with a ready
source and clear loads. The host-only `--network-id` override supports collision
tests.

Run the first automated integration scenario:

```sh
python3 -m sim.scenarios short_recovery
```

The runner starts an isolated Compose project, waits for both firmware nodes to
see a peer and reach `running`, injects a short into each revision's district 0,
checks that power is inhibited without disturbing district 1, clears the short,
and waits for normal operation to resume after ten clear probes. It reports the
last observed state and container logs on failure, then tears down its project.
It uses a test-only TCP control port on each host binary; these commands are
not ARC CAN messages and are absent from embedded firmware.

To exercise collision detection, start the optional third node with a different
UID and node A's ID:

```sh
docker compose -f sim/compose.yaml --profile collision up --build
```

Both nodes with ID `fa5138` report the conflict. Collision recovery is not
implemented.

The bus transport uses newline-delimited JSON internally; this is not an ARC
wire format. It batches frames for 5 ms and orders each batch by CAN ID for
application tests. It does not model bit timing, acknowledgements, CAN errors,
or bus-off.

The host runner uses an initial board-component model. A DRV8874 model derives
fault and current observations from the applied drive and an injected short;
rev1 uses simulated TCA9534 output, direction, and fault-input registers, while
rev2 uses direct GPIO state. The current and fault behavior is intentionally
coarse, and the embedded CAN task is not implemented yet. The embedded and host
HALs are still separate implementations, so hardware register behavior must be
checked against the board design as this model matures.

For full end-to-end validation, the simulated PSU will provide DCC timing and
packets, track and locomotive models will derive load and occupancy from that
signal, and the DRV8874 will turn those effects into sampled current and fault
pins. A deterministic simulation clock and scripted fault injection will make
complete scenarios hermetic and repeatable without physical boards.

Run portable simulator tests with:

```sh
python3 -m unittest discover -s sim/tests
cd firmware && cargo test -p link && cargo test -p dst-01 --lib
```
