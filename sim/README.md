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

Compose starts the clock at zero and leaves it paused. Advance it through the
world control port with `{"op":"advance","milliseconds":100}`; no firmware
timer or CAN batch progresses while it is paused.

Node A builds with `host,rev1` and node B with `host,rev2`. PSU-01 closes the
simulated link switch after its monitor responds and qualifies 15 V. It waits
for a CAN transmit grant before starting the `FF 00 FF` DCC idle stream. Run
layoutd against the virtual CAN bus with all three boards in its retained layout
to select and grant a master. The district nodes validate the packet
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
python3 -m sim.scenarios moving_loco
python3 -m sim.scenarios phase_bridge
```

Each runner starts an isolated Compose project with the same explicit world
clock, including scenarios that leave the physical track fixture disconnected
from district outputs. The original short-recovery
scenario verifies PSU boots without DCC
permission, explicitly grants PSU using the CAN wire message, and waits for both firmware nodes to
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

The `moving_loco` and `phase_bridge` scenarios start a separate physical world from
`sim/world/fixtures/two_districts.json`. This fixture owns actual segment
lengths, endpoint connections and A/B rail mapping, district-output wiring,
and loco placement. It exists independently of `layoutd` and MQTT, so a test
can leave the administrative layout unknown or configure it incorrectly while
the physical system continues to behave. The world receives DCC
packets from the signal bus, passes speed commands only to locos on energized
districts, advances their positions, and gives the DST host current and fault
samples. The scenario checks that CAN current moves from one district to the
other as the loco crosses. A pickup span bridges both outputs near a boundary;
a phase mismatch after accounting for endpoint rail mapping produces a fault
and high current on both powered outputs. `phase_bridge` uses a swapped-rail
fixture and checks that the real DST host controllers trip on both outputs.
Host firmware currently holds phase normal, so automatic phase correction is
not yet exercised.

Run the world outside a scenario with:

```sh
ARC_SIM_WORLD=world:17502 docker compose -f sim/compose.yaml up --build
```

`ARC_SIM_WORLD` connects node A to the physical track fixture;
`ARC_SIM_CAN_CLOCK` puts CAN batches on the world clock; `ARC_SIM_CLOCK`
connects PSU and both DST hosts to that clock. Set `ARC_SIM_CLOCK` for a
separately launched layoutd as well, using `127.0.0.1:17502` from the host.
Runs without `ARC_SIM_WORLD`
use the existing fixed-load district behavior. The world control port is 17502 and accepts one JSON
request per line: `{"op":"status"}`, `{"op":"advance","milliseconds":100}`,
`{"op":"set_turnout","piece":"id","path":"route"}`,
or `{"op":"place","loco":"id","piece":"id","path":"route","offset_mm":10}`.

To exercise collision detection, start the optional third node with a different
UID and node A's ID:

```sh
docker compose -f sim/compose.yaml --profile collision up --build
```

Both nodes with ID `fa5138` report the conflict. Collision recovery is not
implemented.

The CAN transport uses newline-delimited JSON internally; this is not an ARC
wire format. It batches frames for 5 ms and orders each batch by CAN ID for
application tests. In scenario runs the batch boundary is every fifth
logical millisecond, and the clock waits for dispatch before continuing. The
scenario sender uses a test-only queue barrier before advancing time. A separate signal bus broadcasts link power and DCC packets
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

The physical world has one logical clock, starting at zero. DCC packets, output
samples, status requests, and host scheduling do not advance it. A scenario
advances it explicitly with the `advance` request. Live advances run in 1 ms
ticks. At each tick the clock waits for CAN arbitration when due, PSU firmware,
link-signal delivery, ordered district firmware processes, and every registered
output sample before continuing. This prevents a pickup bridge from being skipped.
PSU power qualification, DCC packet cadence, district source timeout, protection
timers, and current-report windows all use this logical time in world scenarios.
Loco position and world time are repeatable for the same tick-ordered inputs. A valid DCC packet marks
the simulated source active until link power or the signal connection is lost;
the DST host still enforces its own 30 ms packet-liveness timeout using logical
time. Socket and process deadlines only detect stalled participants; they do
not advance simulation time. Scripted fault injection remains future work. The model does not resolve electrical waveform
shape, wheel-by-wheel pickup, inertia, or detailed motor physics.

Run portable simulator tests with:

```sh
python3 -m unittest discover -s sim/tests
cd firmware && cargo test -p link && cargo test -p dst-01 --lib && cargo test -p psu-01 --lib
```


PSU and DST rev1 use the shared boot-lifetime DCC permission latch. Both start
without permission; tests use the actual CAN Grant message. Rev2 ignores grants. Source status
reports capability, permission, and actual output. Power loss inhibits output
without clearing permission; source restart clears permission and the table.

The host-process test starts both DST revisions without a PSU, supplies simulated
link power and a logical clock participant, and checks CAN grant targeting, malformed grants, rev2 rejection,
exact idle packets and half-bit timing, and permission surviving power loss.
Build and run it from the repository root:

```sh
(cd firmware && cargo build -p dst-01 --bin dst-01-host --features x4,host,rev1 --target-dir target/host-rev1)
(cd firmware && cargo build -p dst-01 --bin dst-01-host --features x4,host,rev2 --target-dir target/host-rev2)
ARC_DST_REV1_BIN="$PWD/firmware/target/host-rev1/debug/dst-01-host" \
ARC_DST_REV2_BIN="$PWD/firmware/target/host-rev2/debug/dst-01-host" \
python3 -m unittest sim.tests.test_dst_sync
```

The layoutd integration test runs the real daemon with an isolated Docker MQTT
broker and all three host firmware processes. It checks that an absent expected
board blocks permission, completing the layout selects only PSU, layoutd exit
does not revoke permission, and a source restart clears it. Docker must be running.

```sh
(cd services/layoutd && cargo build)
(cd firmware && cargo build -p psu-01 --bin psu-01-host --features host)
ARC_LAYOUTD_BIN="$PWD/services/layoutd/target/debug/layoutd" \
ARC_PSU_BIN="$PWD/firmware/target/debug/psu-01-host" \
ARC_DST_REV1_BIN="$PWD/firmware/target/host-rev1/debug/dst-01-host" \
ARC_DST_REV2_BIN="$PWD/firmware/target/host-rev2/debug/dst-01-host" \
python3 -m unittest discover -s sim/tests
```

The shared `dcc` tests cover packet encoding, table scheduling, continuous bit
sequencing, and permission lifetime. Timer register behavior, interrupt deadlines,
oscillator tolerance, and physical transceiver timing still need hardware checks.
The current signal bus models packet content and liveness, not electrical driver
contention.
