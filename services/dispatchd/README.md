# dispatchd: first bridge slice

`dispatchd` reads district and PSU throttle status from the simulator CAN bus,
publishes operational MQTT state, and translates complete locomotive throttle
commands to ARC-Link CAN. The PSU maintains a locomotive table and reports
its applied state, so `active` is populated after a fresh PSU status arrives.
This service does not yet manage control ownership, warrants, or authority
grants.

Run against the virtual CAN bus and an MQTT broker:

```sh
cargo run --manifest-path services/dispatchd/Cargo.toml -- \
  services/dispatchd/examples/demo.json 127.0.0.1:17500
```

The optional third and fourth arguments are MQTT host and port. `sender_id` is
a configured 24-bit CAN source ID for this initial simulation. Pi identity and
collision recovery remain protocol work. `session` scopes command sequence
numbers and is not an authority grant.
`occupancy_threshold_ma` is a provisional average-current threshold for a
running district; it needs calibration. Non-running or invalid sensing yields
`unknown`, and a three-second status timeout emits an `unknown` update.

Publish a non-retained command:

```sh
mosquitto_pub -t /demo/loco/test/command -m \
  '{"type":"throttle","request_id":"r1","target":{"direction":"forward","speed":20,"stop_mode":"normal"}}'
```

`/demo/loco/test/throttle` is retained and contains exactly `target` and
`active`. `target` is the last accepted complete throttle request; `active` is
the latest fresh PSU status block, or `null`. The status block carries
`observed_at_ms`; clients must age it out if `dispatchd` disconnects before it
can clear its retained snapshot. CAN transmission is not PSU
acceptance. `/demo/loco/test/result` is a non-retained local validation/send
result. Incoming commands are never retained; retained commands are ignored.

`/demo/district/<board-id>-<n>/status` is non-retained, where the board ID is
six lowercase hex digits and `n` is 1–4. It reports `occupancy` as `occupied`,
`clear`, or `unknown`, together with district mode, controller state, current
validity, average/peak current, measurement window, and protection trip. The
MQTT suffix is one-based; the CAN output index is zero-based.

The simulator bus is a TCP line transport for CAN frames. SocketCAN access,
PSU command acceptance, source identity, control handover, and authority
sessions are subsequent integration steps.
