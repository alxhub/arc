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

The `CanNetwork` trait separates CAN observations from the daemon. The initial
backend reads a simulated node snapshot from JSON; a future SocketCAN backend
can implement the same interface after the ARC-Link discovery and policy protocol
is defined. This first version observes board identity but does not issue CAN
configuration commands or verify applied policy.

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
