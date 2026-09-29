# layoutd

`layoutd` owns retained facts at `/<prefix>/layout/node/<id>/fact`. Each fact
describes one track node: its kind, physical board/output binding, and named
connections to other track nodes. It derives and validates the layout graph by
joining these facts. There is no layout configuration file or retained full
layout object.

The retained `/<prefix>/layout/index` is a JSON array of node IDs. It tells
`layoutd` and other subscribers which retained facts to expect. It contains no
topology or version. On startup, `layoutd` waits for every indexed fact,
rebuilds the graph, checks CAN observations, and republishes overall status.
CAN discovery adds observed hardware nodes to the live picture; an observed
board without a matching binding remains unconfigured and prevents `good`.

Each update publishes `updating`, the retained node facts, the ID list, and then
`good`, `invalid`, or `degraded`. Removing a node clears its retained fact and
omits its ID from the list. An interrupted update may leave mixed old and new
facts; this design favors a small, editable layout over automatic transaction
recovery. The status has a retained offline last will. On a fresh broker, the
daemon waits briefly for an index before accepting the first command; the wait
is not used to certify a populated layout as complete.

The `CanNetwork` trait supplies observations and targeted DCC grants. Use
`tcp://host:port` for the virtual CAN bus, or a JSON snapshot for file-based tests.
The file backend appends grant frames to the sibling `*.commands.jsonl` file;
it does not invent an acknowledgement. SocketCAN remains unimplemented.

After the layout is `good` and every observed board has reported its DCC
capability, layoutd selects one master: an already-permitted source first,
otherwise PSU before DST rev1, then lowest network ID. It publishes the selected
six-digit ID at `/<prefix>/layout/dcc_master` (initially JSON `null`) and sends
CAN grants until permission is reported. That retained value is the selection,
not an acknowledgement. The selected node stays fixed; no handover or failover
is performed. Firmware permission persists until source restart without renewal.
If that source restarts, layoutd can re-grant it when the layout is good again.

An expected board without track outputs, such as PSU, uses an `infrastructure`
node fact with configured empty connections and a binding to board output `0`.
All expected boards, including PSU, must be represented; unexpected observations
still prevent `good`. Capability `kind` is 0 (none), 1 (PSU), or 2 (DST rev1).

Run a local MQTT broker, then start the daemon:

```sh
cargo run --manifest-path services/layoutd/Cargo.toml -- \
  demo services/layoutd/examples/sim-can.json
```

Send a non-retained command to create an isolated district:

```sh
mosquitto_pub -t /demo/layout/command/set -m '{"request_id":"init","id":"main","fact":{"kind":"district","topology":{"status":"configured","connections":[]},"binding":{"node_id":"fa5138","output":0}}}'
```

The daemon publishes a non-retained result at
`/<prefix>/layout/command/result`. A null `fact` removes that node. Adding a connection
requires reciprocal port records on both nodes before overall status becomes
`good`. The simulator file represents CAN observations only. The optional third
and fourth daemon arguments set MQTT host and port.

For host firmware on the CAN lab:

```sh
ARC_SIM_CLOCK=127.0.0.1:17502 \
  cargo run --manifest-path services/layoutd/Cargo.toml -- demo tcp://127.0.0.1:17500
```

With `ARC_SIM_CLOCK`, layoutd participates in the simulator's explicit world
ticks. CAN presence expiry, source expiry, bootstrap timeout, and master
selection use logical milliseconds. The simulator waits for layoutd to process
each tick before advancing. Broker connection deadlines still protect the
socket; they do not move logical time. Omit this variable when running without
the world service.

For example, an expected PSU fact is:

```json
{"kind":"infrastructure","topology":{"status":"configured","connections":[]},"binding":{"node_id":"123456","output":0}}
```

Its node ID must be included in the retained layout index just like district
facts. A `good` layout with no transmitter remains without a selected DCC master.
