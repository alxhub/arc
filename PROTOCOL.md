# ARC CAN protocol

This document defines the ARC-Link CAN wire space. The defined messages are
Presence, District Configuration, District Status, PSU Status, Locomotive Throttle Set, Locomotive Throttle Status, DCC Grant, DCC Status, and firmware update transfer. Future messages must use the common frame rules below and receive an
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
current board implementation uses 500 kbit/s for arbitration and data without
bit-rate switching; this timing still needs hardware validation. The arbitration
identifier is composed as follows:

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
| `0x00`–`0x07`, `0x09`–`0x0f` | Unassigned; lower values have higher bus priority. |
| `0x08` | Locomotive throttle control. |
| `0x10` | District control. |
| `0x11` | District status. |
| `0x12` | PSU status. |
| `0x13` | Locomotive throttle status. |
| `0x14` | DCC transmit grants. |
| `0x15` | DCC capability and permission status. |
| `0x16` | Firmware update control and image data. |
| `0x17` | Firmware update acknowledgements. |
| `0x18`–`0x1e` | Unassigned. |
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
Within discovery class `0x1f`, locomotive throttle control class `0x08`,
district control class `0x10`, district status class `0x11`, PSU status class
`0x12`, and locomotive throttle status class `0x13`, types `0x00`
and `0x02`–`0xff` are unassigned.

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

## District control message `0x01`: Configuration

Configuration uses class `0x10` and contains exactly 7 data bytes (CAN FD DLC
7). It replaces the commanded state of one output. Districts boot
disabled, and this command is the only defined CAN path to select an active
mode. The target board ID is in the payload because the arbitration ID identifies
the sender. An unmatched target is ignored.

| Offset | Size | Field | Value |
| --- | ---: | --- | --- |
| 0 | 1 | Protocol major version | `0x01` |
| 1 | 1 | Message type | `0x01` |
| 2 | 3 | Target board network ID | Big-endian 24-bit ID |
| 5 | 1 | District index | `0`–`3` on DST-01x4 |
| 6 | 1 | Commanded state | `0` disabled, `1` active on backbone synchronized DCC, `2` programming |

The receiver rejects the entire frame if its length, envelope, commanded state,
or district index is invalid. Recovery timing is local firmware policy; this
message does not configure it. Repeating the same command
does not restart recovery or clear short backoff. A changed configuration gates
the output off before using a different source or range. A disabled command
clears a latched hardware I/O fault; enabling afterward starts recovery again.

Selecting an active mode does not by itself energize the rails. The selected DCC
source must also be ready, and the district's local protection controls startup
probes and short recovery. Configurations are volatile and must be sent again
after a node restarts. Sender authorization, command acknowledgements, and
collision-safe targeting are still to be defined; a
Presence announcement alone does not make a target unambiguous after an ID
collision.

## District status message `0x01`: Status

Each DST-01x4 output publishes a status in class `0x11` roughly once per second
and whenever its selected mode, actual controller state, or protection-trip flag
changes. State-change reports do not postpone the periodic reports.
Its arbitration ID contains the reporting board's network ID. A status reports
the actual controller state, the selected operating mode, and current measured
over the preceding window. It is a periodic observation rather than a CAN
acknowledgement for a particular command.

| Offset | Size | Field | Value |
| --- | ---: | --- | --- |
| 0 | 1 | Protocol major version | `0x01` |
| 1 | 1 | Message type | `0x01` |
| 2 | 1 | District index | `0`–`3` |
| 3 | 1 | Selected mode | `0` disabled, `1` active, `2` programming |
| 4 | 1 | Actual state | `0` disabled, `1` waiting for source, `2` probing, `3` cooling down, `4` running, `5` hardware fault |
| 5 | 1 | Flags | Bit 0: protection trip detected during current recovery; bit 1: current measurement valid; other bits zero |
| 6 | 2 | Average current | Milliamps, unsigned big-endian, capped at 65,535 |
| 8 | 2 | Peak current | Milliamps, unsigned big-endian, capped at 65,535 |
| 10 | 4 | Measurement window | Elapsed milliseconds since this district's previous status, unsigned big-endian, nonzero |
| 14 | 2 | Reserved | Zero |

The payload is exactly 16 bytes (CAN FD DLC 10). Average current is time
weighted across the stated window, including time when the output is off. Peak
is the highest sampled current in that window. A protection trip remains flagged
through recovery until continuous drive resumes or the district is disabled.
The flag means overcurrent or a driver fault was observed; it does not identify
the precise electrical cause. If current
measurement is unavailable, the validity flag is clear and both current fields
are zero. Status reports the local controller's observation and may lag a recent
command by up to the next controller tick and bus delivery time. A state-change
report can have a shorter current-measurement window than a periodic report.

## PSU status message `0x01`: Status

PSU-01 publishes status in class `0x12` approximately once per second while
the link is online. It also publishes when its state changes to online. Its
arbitration ID contains the PSU's network ID. The payload is exactly 12 bytes
(CAN FD DLC 9):

| Offset | Size | Field | Value |
| --- | ---: | --- | --- |
| 0 | 1 | Protocol major version | `0x01` |
| 1 | 1 | Message type | `0x01` |
| 2 | 1 | Actual state | `0` off, `1` starting, `2` online, `3` fault latched |
| 3 | 1 | Flags | Bit 0: DCC line driver enabled; bit 1: monitor reading valid; other bits zero |
| 4 | 2 | Link voltage | Millivolts, unsigned big-endian |
| 6 | 2 | Link current | Milliamps, unsigned big-endian |
| 8 | 4 | Uptime | Milliseconds since PSU boot, unsigned big-endian, saturating |

If the monitor reading is invalid, both measurement fields are zero. A link
fault can remove power from the receivers before a fault report can be
delivered; the status is an observation, not a guarantee of fault delivery.

## Locomotive throttle control message `0x01`: Set

Set uses class `0x08` and contains exactly 20 data bytes (CAN FD DLC 11).
It replaces one DCC-source throttle-table entry with a complete state. The target source
network ID is in the payload. The sender ID is in the arbitration identifier.

| Offset | Size | Field | Value |
| --- | ---: | --- | --- |
| 0 | 1 | Protocol major version | `0x01` |
| 1 | 1 | Message type | `0x01` |
| 2 | 3 | Target DCC source network ID | Big-endian 24-bit ID |
| 5 | 4 | Command session | Big-endian sender session identifier |
| 9 | 4 | Sequence | Big-endian per-locomotive sequence within the session |
| 13 | 1 | Address kind | `0` short, `1` long |
| 14 | 2 | DCC address | Unsigned big-endian; address validity depends on kind |
| 16 | 1 | Direction | `0` reverse, `1` forward |
| 17 | 1 | Speed step | `0` stop, `1`–`126` motion |
| 18 | 1 | Stop mode | `0` normal, `1` emergency; emergency requires speed `0` |
| 19 | 1 | Reserved | Zero |

The source rejects malformed values, an unmatched target, and a sequence older
than the last accepted sequence for that locomotive within the same command
session. An exact repeat is idempotent and causes another status report. A new
session starts a new sequence space for that locomotive. The session currently
provides correlation and ordering only; it is not an authority grant. A status
report, not CAN transmission, confirms application. DCC transmission requires the separate boot-lifetime grant below.
Sender authentication remains future protocol work.

## Locomotive throttle status message `0x01`: Status

Status uses class `0x13` and contains exactly 20 data bytes (CAN FD DLC 11).
The arbitration identifier identifies the reporting DCC source. It reports a table
entry when that entry changes and periodically while it remains active.

| Offset | Size | Field | Value |
| --- | ---: | --- | --- |
| 0 | 1 | Protocol major version | `0x01` |
| 1 | 1 | Message type | `0x01` |
| 2 | 4 | Command session | Session of last accepted Set |
| 6 | 4 | Sequence | Sequence of last accepted Set |
| 10 | 1 | Address kind | `0` short, `1` long |
| 11 | 2 | DCC address | Unsigned big-endian |
| 13 | 1 | Direction | `0` reverse, `1` forward |
| 14 | 1 | Speed step | `0`–`126` |
| 15 | 1 | Stop mode | `0` normal, `1` emergency |
| 16 | 1 | State | `0` held from DCC transmission, `1` eligible for DCC transmission |
| 17 | 3 | Reserved | Zero |

This reports the source's effective DCC table state. It does not measure decoder
reception or locomotive motion. A receiver must treat status as stale after its
reporting timeout; old status must never restore a throttle request.


## DCC control message `0x01`: Grant

PSU-01 and DST-01 rev1 obey the same transmit permission. Both boot without
permission and keep their DCC line drivers disabled, including idle packets,
until a targeted grant arrives. The grant latches in RAM until the source
restarts. There is no revoke, lease, renewal, handover, or automatic failover.
Repeated grants are idempotent. CAN or layoutd disappearance does not clear it.
Local power qualification still gates actual transmission and does not clear
the permission. District configuration remains independent.

Grant uses class `0x14`, with the sender's network ID in the arbitration ID,
and exactly five data bytes (CAN FD DLC 5):

| Offset | Size | Field | Value |
| --- | ---: | --- | --- |
| 0 | 1 | Protocol major version | `0x01` |
| 1 | 1 | Message type | `0x01` |
| 2 | 3 | Target source network ID | Big-endian 24-bit ID |

Only a capable node matching the target accepts the grant. DST rev2 ignores it.
The current virtual-CAN layoutd uses sender ID `000001`, matching host test
commands; this is a configured lab identity, not a hardware identity derivation.
There is no sender authentication in this protocol version.

## DCC status message `0x01`: Capability and permission

Every implemented board publishes class `0x15` at startup, on changes, and
approximately once per second while CAN is available. The arbitration ID
identifies the reporting node. Exactly four data bytes (CAN FD DLC 4):

| Offset | Size | Field | Value |
| --- | ---: | --- | --- |
| 0 | 1 | Protocol major version | `0x01` |
| 1 | 1 | Message type | `0x01` |
| 2 | 1 | Source kind | `0` no transmitter, `1` PSU, `2` district transmitter |
| 3 | 1 | Flags | Bit 0: permission granted; bit 1: actually transmitting; other bits zero |

A node without a transmitter reports both flags clear. Transmitting requires
permission. Permission may remain set while the output is inhibited by power
qualification. This reports the local line driver, not receiver readiness or
locomotive reception. PSU still supplies link power and CAN discovery before
permission, so layoutd can discover all nodes before allowing DCC.

Layoutd grants only after the layout is `good`: its graph validates and every
expected hardware identity is present, without unexpected or duplicate nodes.
It also waits for all observed nodes to report their DCC capability. It reuses
an already-permitted source if present, otherwise prefers PSU over DST rev1,
then the lowest network ID. It keeps that selection and retries the same grant
once per second until status confirms permission. These are delivery retries,
not renewals. It never reassigns a missing master. After a source restart,
layoutd can grant that same node again when the layout is good.

Throttle Set/Status target and identify the selected DCC source using the
existing formats. Preparing a throttle table never enables the line driver;
source restart clears both permission and the volatile table.

## Firmware update over CAN FD

Classes `0x16` (sender to board) and `0x17` (board acknowledgement) use the
same extended-ID source rule and envelope as other ARC frames. The sender must
use a configured, unique 24-bit network ID. All messages target one board by
the network ID learned from Presence. Only one transfer may be active per board.

The image is a raw application binary linked for the Embassy active partition
at `0x08004000`; its first eight bytes are the little-endian Cortex-M stack
and reset vectors. The bootloader at `0x08000000` is installed separately.
Image kind is `1` for DST-01x4 rev1, `2` for DST-01x4 rev2, and `3` for PSU-01.
The sender computes CRC-32/ISO-HDLC (the same as Python `zlib.crc32`) over the
exact unpadded image. CRC checks transfer integrity; this protocol has no
sender authentication or image signature in version 1.

### Control type `0x01`: Begin

Exactly 20 bytes (CAN FD DLC 11):

| Offset | Size | Field |
| --- | ---: | --- |
| 0–1 | 2 | Version `1`, type `1` |
| 2–4 | 3 | Target network ID |
| 5 | 1 | Image kind |
| 6–9 | 4 | Nonzero transfer session |
| 10–13 | 4 | Image size in bytes |
| 14–17 | 4 | CRC-32 |
| 18–19 | 2 | Zero |

An accepted Begin erases the staging partition and returns Ready with next
offset zero. Repeating Begin restarts the transfer. The board rejects an image
larger than its active partition (240 KiB on rev1; 54 KiB on rev2 or PSU).

### Control type `0x02`: Chunk

Exactly 64 bytes (CAN FD DLC 15):

| Offset | Size | Field |
| --- | ---: | --- |
| 0–1 | 2 | Version `1`, type `2` |
| 2–4 | 3 | Target network ID |
| 5–8 | 4 | Session |
| 9–12 | 4 | Zero-based image offset, a multiple of 48 |
| 13 | 1 | Valid data length, 1–48 |
| 14–61 | 48 | Data, followed by `0xff` padding |
| 62–63 | 2 | `0xff 0xff` |

Each chunk except the last carries 48 valid bytes. The board writes valid
bytes, padded to its eight-byte flash write unit, then acknowledges the next
expected offset. A duplicate of an already accepted chunk gets the current
offset again. An out-of-order chunk is rejected. The sender waits for each
acknowledgement before sending the next chunk and retries on timeout.

### Control type `0x03`: Finish

Exactly 12 bytes (CAN FD DLC 9): version `1`, type `3`, target ID (3 bytes),
session (4 bytes), and three zero bytes. Once the full image is present, the
board reads staging flash back, checks CRC-32, checks that stack and reset
vectors address its RAM and active image, marks the Embassy update pending,
acknowledges Rebooting, and resets. Embassy swaps the image at boot. The new
application confirms its trial boot after initialization; Embassy can restore
the previous image if that confirmation never happens.

### Status type `0x01`: Acknowledgement

Exactly 12 bytes from class `0x17` (CAN FD DLC 9): version `1`, type `1`,
session (4 bytes), next expected offset (4 bytes), phase (1 byte), error
(1 byte). Phase is `1` Ready, `2` Progress, `3` Rebooting, or `4` Rejected.
Error zero means success; rejection errors are `1` wrong kind/size, `2` flash,
`3` session, `4` offset/length, `5` CRC, and `6` vector address. The board's
network ID in the arbitration identifier identifies the responder.

During staging, DST disables its districts and rev1 DCC transmission. PSU
keeps ARC-Link and CAN powered but inhibits its DCC transmitter. An interrupted
transfer leaves the current application intact; send Begin again to retry.
