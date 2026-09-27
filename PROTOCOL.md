# ARC CAN protocol

This document defines the ARC-Link CAN wire space. The first defined message is
Presence. Future messages must use the common frame rules below and receive an
explicit class and type assignment here before implementation. The control
responsibilities that these messages serve are described in [CONTROL.md](CONTROL.md).

## Network identity

Every ARC CAN data frame carries the transmitting node's 24-bit network ID in
the low 24 bits of its extended arbitration identifier. Network IDs are written
as exactly six lowercase hexadecimal digits in MQTT, configuration, and
diagnostics, with leading zeroes when needed and no `0x` prefix. A network ID
identifies a board, not an individual district output; an output is identified
by its board ID and output index.

An STM32 node obtains the 12 UID bytes at successive addresses of its MCU's
96-bit unique-device-ID register. Its initial network ID is the low 24 bits of
the 32-bit FNV-1a hash of the five ASCII bytes `stm32`, followed immediately by
those 12 UID bytes in increasing register-address order. There is no separator
or terminator:

```text
hash = 0x811c9dc5
for byte in ASCII("stm32") || uid[0..12]:
    hash = ((hash XOR byte) * 0x01000193) mod 2^32
network_id = hash AND 0x00ffffff
```

For example, UID bytes `00 01 02 03 04 05 06 07 08 09 0a 0b` produce network
ID `fa5138`. The byte order of the UID is defined by its register addresses,
not by the MCU's integer endianness. Both DST-01x4 revisions use this same rule.

A 24-bit hash can collide. Presence carries the full device identity so a
receiver can distinguish nodes that proposed the same network ID. A node may
eventually resolve a collision by choosing a random replacement ID and sending
a new Presence. The selection, tie-breaking, persistence, and reannouncement
procedure is not defined yet; firmware must not silently merge distinct device
identities while this path is absent. Identity rules for non-STM32 transmitters,
including the Pi, remain to be assigned.

## CAN frame space

ARC-Link uses CAN FD data frames with 29-bit extended arbitration identifiers.
Remote frames and 11-bit standard identifiers are outside this protocol. The
arbitration identifier is composed as follows:

| Bits | Meaning |
| --- | --- |
| 28–24 | Message class, from `0x00` through `0x1f`. |
| 23–0 | Transmitting node's 24-bit network ID. |

Lower arbitration IDs win bus access. Message class therefore sets bus priority
before the sender ID breaks ties among different nodes in that class. Classes
are assigned to related message types, allowing a receiver to filter by class
in its CAN controller. The class is not a destination address: ARC CAN messages
are broadcast, and any narrower intended audience belongs in the message's
payload and semantics.

| Class | Assignment |
| --- | --- |
| `0x00`–`0x1e` | Unassigned; lower values have higher bus priority. |
| `0x1f` | Discovery, including Presence. |

For example, node `fa5138` sends Presence under extended CAN ID `0x1ffa5138`.
Only assigned message types may use a class; future assignments must specify
both the class and payload message type.

Two nodes with the same network ID also have the same arbitration identifier
when sending in the same class. If they transmit different frames simultaneously,
CAN arbitration cannot distinguish them and a frame error can result. Presence
makes the collision observable once each announcement is received, but collision
avoidance and recovery still need a defined procedure.

All multibyte integers in payloads are unsigned and big-endian. The source
network ID is obtained from arbitration bits 23–0 and is not repeated in the
payload. All frames start with the following two-byte envelope:

| Offset | Size | Field | Value |
| --- | ---: | --- | --- |
| 0 | 1 | Protocol major version | `0x01` |
| 1 | 1 | Message type | Assigned below |

Receivers discard frames with an unsupported major version. A message format is
identified by its `(class, type)` pair; type values are scoped to their class.
Within discovery class `0x1f`, types `0x00` and `0x02`–`0xff` are unassigned.
No operational command, policy, acknowledgement, or status format is implied by
this initial assignment.

## Message `0x01`: Presence

Presence announces the identity behind a network ID. It uses discovery class
`0x1f` and contains exactly 16 data bytes (CAN FD DLC 10):

| Offset | Size | Field | Value |
| --- | ---: | --- | --- |
| 0 | 1 | Protocol major version | `0x01` |
| 1 | 1 | Message type | `0x01` |
| 2 | 1 | Device class | `0x01` for `stm32` |
| 3 | 1 | Flags | `0x00`; all bits reserved |
| 4 | 12 | Device UID | Raw STM32 UID bytes in increasing register-address order |

Presence uses the current network ID in its arbitration identifier; that ID may
eventually differ from the initial hash after collision recovery. A receiver
identifies a physical node by the pair `(device class, device UID)`. If two
Presence messages claim one network ID for different physical identities, the
receiver reports a collision and treats neither claim as an unambiguous control
target.

An STM32 node sends Presence when it joins the bus and periodically while it is
online. The cadence, liveness timeout, and collision-recovery exchange are still
to be specified. Presence announces identity; it does not grant authority, prove
a district is safe, or establish that configuration has been applied.

For the example UID above, the Presence frame is:

```text
extended arbitration ID: 0x1ffa5138
data (16 bytes):          01 01 01 00 00 01 02 03 04 05 06 07 08 09 0a 0b
```
