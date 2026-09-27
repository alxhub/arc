# ARC control architecture

This document captures the intended responsibilities and interactions of ARC's
control planes. It is an architecture draft, not a description of completed
firmware or a CAN wire-protocol specification. See [README.md](README.md) for
the system's physical organization, [PROTOCOL.md](PROTOCOL.md) for the ARC CAN
wire format, and the board READMEs for implementation status.

## Planes and responsibilities

ARC separates four responsibilities. The layout computer, normally a Raspberry
Pi, hosts both the administrative and operations planes. The CAN network forms
the control plane, and individual nodes form the field plane.

| Plane | Responsibility |
| --- | --- |
| Administrative | Provides interfaces to the outside world; grants control authority and distributes configuration and policy to nodes. |
| Operations | Uses layout-wide knowledge to track locomotives, automate movements, manage throttles, and anticipate conflicts or needed hardware changes. |
| Control | Carries events and coordination over CAN; nodes report and respond according to their configured policies. |
| Field | Owns physical interfaces, sensing, actuation, and local electrical protection. |

These are divisions of responsibility, not separate devices or physical network
tiers. A node participates in the control plane while implementing its physical
behavior in the field plane.

### Administrative plane

The administrative plane establishes what nodes are allowed to do and the bounds
within which they operate. It configures behavior when authority or communication
is lost. Nodes execute that policy without requiring the Pi to referee every
incident.

Authority may have a fixed lifetime and require periodic renewal. Its expiry
behavior is part of the policy granted in advance, rather than a decision that
must be obtained from an unavailable computer.

### Operations plane

The operations plane holds the layout-wide picture: track connections, turnout
positions, district occupancy, and locomotive movements. It may provide scripted
loco automation and PTC-type services, adjusting throttles or applying brakes
when a locomotive approaches an occupied block, a misaligned switch, or another
conflict.

It can also anticipate actions that nodes would otherwise handle reactively.
For example, the Pi can track a locomotive through reported district occupancy
and request the appropriate output phase before it crosses a reversing-loop
boundary. This improves operation without being required for local electrical
safety. If the prediction is absent or wrong, district protection and permitted
reactive recovery remain responsible for the electrical response.

Movement-conflict protection and electrical protection are distinct. Local short
protection does not replace an operations service that prevents conflicting
movements. The response to loss of operational control is administrative policy;
the PSU authority mechanism below provides the DCC fallback.

### Control plane

Nodes publish events and respond to track observations and CAN events under their
policies. Coordination can occur directly between nodes without a round trip
through the Pi.

District MCUs do not maintain track adjacency or route models. The Pi translates
its layout knowledge into narrower rules for each district: the bounds of its
behavior and how it should respond to specified events. Referencing a CAN event
or another node does not require understanding the physical track relationship
behind that rule.

### Field plane

Each node has ultimate responsibility for electrical safety at its interfaces.
A district node inhibits an output when it detects a short without waiting for
CAN traffic or Pi approval. It may subsequently coordinate over CAN to determine
whether policy permits an autoreversing response and a protected retry.

Administrative authority, operational requests, and peer coordination never
override a local electrical protection constraint.

## Explicit state

State instructions express explicit desired values, never an operation relative
to the receiver's current state. For example:

- Set a district to enabled or disabled; do not toggle it.
- Set phase to normal or inverted; do not command a relative phase flip.
- Set a locomotive's requested throttle state; do not increment its current speed.

Repeating the same state instruction must not repeat a relative action. Events
may describe transitions, but the resulting state instructions remain explicit.

Policy, requested state, and observed state have distinct meanings:

| Kind | Meaning |
| --- | --- |
| Policy | Permitted behavior, constraints, and responses to events or loss of authority. |
| Requested state | The explicit operating state requested by an authorized controller. |
| Observed state | The actual state, including local protection, recovery, and reasons an output is inhibited. |

Acceptance of a request does not imply that an output is delivering power. A
district may have applied a requested mode while waiting for a valid DCC source
or recovering from a fault.

## District behavior and coordination

DST-01x4 presents the same logical district behavior for both hardware revisions.
Revision-specific GPIO, expander access, and signal polarities belong in the
hardware implementation. Hardware revision and capabilities can be reported
without requiring separate control semantics for each revision.

The Pi issues a policy for each district. That policy defines its operating
bounds and its responses to local track events and CAN events. Coordination rules
must settle which participant may change phase and which must hold, avoiding
multiple districts repeatedly reversing together.

A reactive short response follows this division of responsibility:

1. The district detects a fault and locally inhibits its output.
2. The node reports the event and performs any coordination required by policy.
3. If permitted, it applies an explicit phase and conducts a locally protected retry.
4. It reports the resulting state so peers and the Pi can update their observations.

Proactive operations requests and autonomous recovery both feed the district's
decision process. Their precedence must be explicit in policy; competing writers
must not repeatedly undo each other's phase decisions. The precise arbitration
and stale-message rules remain protocol design work.

Pi presence is not itself a prerequisite for district power. A district follows
its policy, source-readiness requirements, and local protection. Loss of Pi
communication, loss of CAN communication, loss of backbone power, and loss of a
valid DCC signal are different conditions, not a single undifferentiated offline
state.

## PSU authority and loss of operational control

The PSU node is the DCC synchronization master. It maintains the locomotive
throttle table and periodically broadcasts DCC packets representing those states
on the backbone's DCC pair. District nodes use that signal to drive their outputs.

The administrative plane grants the PSU authority to pass DCC traffic for a fixed
lifetime and periodically renews that authority. Each grant includes a last-will
instruction (LWT) specifying what the PSU must do if renewal stops arriving.
The PSU measures expiry locally, so either disappearance of the administrative
plane or failure of the CAN control plane can trigger the fallback without any
further communication.

The agreed last-will choices are:

| Last will | Behavior after authority expires |
| --- | --- |
| Continue current states | Keep generating DCC from the throttle states held at expiry. Fresh throttle changes require valid authority. |
| Track-wide emergency stop | Generate track-wide emergency-stop traffic until authority is re-established; leave the effective throttle state stopped. |

Expiry is a transition into the fallback authorized by the last will. In
particular, the continue policy permits continued transmission after the normal
grant expires. Neither fallback requires the Pi or CAN to remain available.

Renewing authority after an emergency stop permits fresh operational commands.
It must not automatically restore old throttle speeds or resume locomotives from
stale table entries. Re-establishing authority alone does not command movement.

An emergency-stop DCC stream remains a valid source signal. Districts can keep
their rails energized to deliver it, subject to their own policies and electrical
protection. The PSU's expiry response is therefore distinct from disabling
district outputs or losing the DCC source entirely.

The last will covers loss of renewal while the PSU remains able to execute it.
PSU reset, startup without a grant, and persistence of authority or throttle data
need explicit lifecycle rules; they are not implied by the continue policy.

## Pi software services

The Pi software separates administrative configuration, operational state,
traffic planning, movement protection, and command sources. These are distinct
ownership boundaries even when all services run on one Pi.

| Service | Owns | Interfaces |
| --- | --- | --- |
| `layoutd` | Layout topology, district-to-track mappings, administrative policy, and node authority grants and renewal. | CAN; layout configuration and publication to other services. |
| `dispatchd` | The authoritative operational picture, locomotive control ownership, and translation of permitted requests into track directives. | CAN and MQTT. |
| `routingd` | Journey planning, reservations, passing maneuvers, and traffic flow. | Software service interfaces; movement requests flow through `dispatchd`. |
| `ptcd` | Movement protection and issuance of track warrants. | Consumes the operational picture and publishes warrants over MQTT. |
| `controllerd` | Physical throttle peripherals and throttle protocol sessions, including Stream Deck and WiThrottle interfaces. | Peripheral interfaces and MQTT; no CAN. |
| `automationd` | Script execution, journey intent, and script progress. | MQTT; no CAN. |

`layoutd` supplies the layout definition; `dispatchd` interprets observations
against it. `routingd` and `ptcd` consume the same operational picture rather than
maintaining competing authoritative estimates of train positions. A mapping
change makes earlier operational interpretations stale; consumers must wait for
the layout to return to `good` and rebuild their picture.

The intended benefit is independent evolution of these responsibilities: a new
planner or automation language can use the same movement protection and hardware
interfaces. Separate processes also introduce restart, freshness, and partial
failure cases that their contracts must address.

### CAN access and MQTT

The HAT's CAN controller is exposed to Linux through SocketCAN. Multiple processes
can independently open sockets on the same CAN interface, with matching received
frames delivered to each interested socket. A single userspace process does not
need exclusive ownership of CAN communication. See the
[Linux SocketCAN documentation](https://docs.kernel.org/networking/can.html).

`layoutd` and `dispatchd` have separate reasons to use CAN. Monitoring tools may
also observe traffic independently. Shared access does not confer shared command
authority: discovery, collision handling, and protocol sessions still need
explicit owners. The hardware-independent ARC-Link `link` crate provides the
home for shared message definitions and codecs.

MQTT carries operational requests and published state between services. Requested
throttle state, effective commanded state, and observed physical movement are
distinct. Controllers display the effective result even when it differs from a
request because movement authority restricts it.

`layoutd` is the sole publisher of retained layout facts at
`/<layout>/layout/node/<id>/fact`. Each fact describes one district or turnout,
its named connections, and any physical board/output binding; an unconfigured
entity has an explicit `unknown` topology. `layoutd` derives the graph from these
facts. A retained index is a plain list of node IDs, giving subscribers a way
to know which retained facts to collect; it does not store topology or version
them. CAN discovery contributes observed hardware nodes.
`layoutd` publishes overall `updating` status before changing records, then
publishes `good` only after topology validation, expected CAN node presence,
and broker acknowledgement of the new records. An interrupted update may leave
partially changed facts; recovery can require another explicit update. The index
defines the expected fact set even if status is `invalid` or `degraded`;
consumers require both the indexed facts and the status appropriate to their task.
MQTT publication alone does not prove a subscriber has received them. Loss of a
node or divergence from configured state changes the overall status, and a
retained MQTT last will marks `layoutd` offline on connection loss.

Manual controllers and automation use the same operational request path.
`dispatchd` arbitrates locomotive control ownership and handover so a script and
a throttle cannot silently compete to drive the same locomotive. The PSU remains
the owner of the DCC throttle table and periodic packet generation; `dispatchd`
owns operational intent and tracks the PSU's applied state.

### Traffic planning and movement protection

An automation script expresses a journey such as "move this train from A to B at
speed X." The requested speed is subject to the route and movement authority.
The script does not need to select each turnout or solve conflicts with other
scripts. `routingd` considers competing journeys together and chooses routes,
waiting points, and passing opportunities.

`routingd` manages traffic flow; `ptcd` prevents conflicting authorized movements.
A planned route or reservation is not permission to move. `ptcd` evaluates the
operational picture and issues a track warrant that bounds what `dispatchd` may
command. It is a separate authority issuer, not an inline rewriter of throttle
messages between MQTT and CAN.

When configured to require warrants, `dispatchd` requires both a throttle request
and valid movement authority. A warrant permits movement but does not itself
request motion. It must express constraints `dispatchd` can enforce; the exact
representation of direction, speed, route limits, and braking remains design work.

For two trains approaching in opposite directions, `routingd` may revise a
journey to put one train in a passing siding and hold it while the other passes.
It must account for train extent, siding capacity, and whether the maneuver is
still reachable. Turnout position and clearance must be established before the
corresponding movement is authorized.

Uncommitted plans can change freely. Changes affecting already authorized
movement require a coordinated transition: a revised route cannot throw a turnout
under an approaching train or grant a conflicting movement before the old
authority has been safely withdrawn. If a siding is no longer reachable within
the movement constraints, the planner must choose another meet or hold traffic
elsewhere. A poor plan may cause delay or deadlock; it must not bypass protection.

### Warrants and service failure

Track warrants belong to an identified `ptcd` session. Its MQTT last will revokes
that session's movement authority, allowing `dispatchd` to invalidate all warrants
from that issuer without requiring a separate last-will message for every train.
A restarted issuer must establish fresh authority; retained or delayed messages
from an old session must not restore it.

MQTT last-will delivery depends on broker detection of connection loss and any
configured Will Delay. Normal graceful disconnection suppresses the will, so an
orderly shutdown must explicitly revoke authority. See the
[MQTT 5.0 specification](https://docs.oasis-open.org/mqtt/mqtt/v5.0/mqtt-v5.0.html).

Warrants also need bounded validity enforced locally by `dispatchd`. This covers
undelivered last wills and a stalled protection process whose MQTT connection
remains alive. Renewal must represent continued protection decisions, not merely
transport liveness. Exact timing and freshness rules remain to be specified.

Revoking movement authority actively changes the effective throttle state.
Simply ignoring new throttle requests is insufficient because the PSU keeps
broadcasting its existing table. `dispatchd` commands the stop response selected
by policy. Fresh authority alone must not resume a stale movement request;
recovery requires an explicit fresh request or re-arm.

| Failure | Intended response |
| --- | --- |
| `ptcd` disappears or warrants expire | `dispatchd` invalidates the affected authority and commands the configured stop response. |
| MQTT communication is lost | `dispatchd` applies the configured communication-loss response; local warrant expiry bounds continued permission. |
| `dispatchd` fails | `layoutd` stops renewing PSU authority when policy requires dispatch health. |
| Pi or CAN disappears | PSU authority expires and its configured last will takes effect. |

`layoutd` must not equate its own process health with operational health.
Administrative policy determines which services must remain healthy for PSU
authority renewal. This preserves the choice between continued operation and
emergency stop established by the PSU last will.

## Operational picture and reconciliation

`dispatchd` builds the operational picture from CAN observations and the layout
definition. It owns train identity association, estimated location and direction,
train extent, and interpreted occupancy. Other services may supply information
or request corrections, but consume `dispatchd`'s published interpretation.

Observations and estimates remain distinguishable. "District 7 draws current"
is an observation. "Train A's locomotive is in district 7 and its tail may still
occupy district 6" is an estimate. The picture must expose uncertainty, freshness,
and the layout version used, including unidentified occupancy and unknown train
locations.

A throttle command or warrant is not evidence of physical movement. Position
updates come from observations and explicitly identified estimates. Locomotive
current detection alone cannot establish that the entire train has cleared a
block; unpowered cars may remain behind. Train extent and braking estimates need
defined inputs and conservative handling of uncertainty.

### Human intervention: the "hand of god"

An operator may pick up a derailed locomotive and place it in another district.
This invalidates the expected movement history. Reconciliation is a normal
operational behavior, not an exceptional repair of the layout database.

The intended automatic response is:

1. A district containing a known locomotive shorts and subsequently returns with
   no detected load. Once valid sensing establishes the disappearance,
   `dispatchd` immediately clears that locomotive's throttle state at the PSU and
   marks its location unknown.
2. Another district draws current without a corresponding plausible arrival of a
   tracked locomotive. `dispatchd` marks the load unidentified and may request
   that the district switch to programming mode to probe locomotive identity.
3. The district executes the identification operation under its policy and local
   electrical protection. A confirmed result lets `dispatchd` reconcile the
   locomotive's location. No response, ambiguous results, and hardware failures
   must remain distinguishable from successful identification.
4. The old journey and movement authority remain suspended pending reassessment.
   Recognition of the locomotive does not restore its previous throttle state.

Zero current while an output is inhibited, cooling down, or missing its DCC source
does not establish disappearance. Occupancy reports need sensing validity and
district state. Likewise, a missing locomotive does not prove its former track
is clear of cars; uncertainty about train extent must remain visible to `ptcd`.

Programming-mode identification is an intended capability, not an implemented
guarantee. The operation must establish suitable conditions rather than assume
an unexplained load is exactly one locomotive or that its district is electrically
separate from neighboring track. Probe eligibility, identification semantics,
and failure handling remain to be designed and validated.

Useful tracking states are tracked, missing, unidentified, and reconciling.
Operator declarations such as "I moved locomotive A here" or "I removed this
train" provide another reconciliation input, with the same stopped-state and
authority reassessment behavior as automatic discovery.

## Remaining protocol decisions

This architecture establishes responsibilities. Further design must define:

- CAN message formats beyond Presence, protocol evolution, and collision recovery
  (see [PROTOCOL.md](PROTOCOL.md) for the defined wire format).
- Policy representation and the precedence of operational requests and local recovery.
- Authority sessions, renewal timing, and rejection of stale commands or renewals.
- Peer coordination exchanges, timeouts, and behavior when coordination fails.
- State acknowledgements, event reporting, and periodic status snapshots.
- Startup and reset behavior, including policy persistence and PSU throttle initialization.
- MQTT request/state schemas, issuer sessions, warrant validity, and service health contracts.
- Layout change transitions, train extent estimation, and operational-picture freshness.
- Locomotive control handover, journey lifecycle, and coordinated route/authority changes.
- Ownership of turnout execution and locking during route establishment and release.
- Identification-probe eligibility, results, and operator-assisted reconciliation.

These details belong in the Pi service contracts, shared ARC-Link protocol, and board implementations
without moving local electrical protection into the network or requiring district
nodes to understand the layout's track topology.
