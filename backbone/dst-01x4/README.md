# ARC-DST-01x4 rev 2

ARC-DST-01x4 drives four track districts. It receives the shared DCC timing signal from the ARC backbone, then lets the controller set each district's phase and turn each output on or off independently. A district can also switch to a separate programming signal with a lower current limit.

The board has its own connection to the layout's track supply. It reports total track current and supply voltage to the controller, while each district reports its own current and fault state. That lets the system identify and respond to a problem in one section without treating all four sections as one output.

CAN connects the board to the rest of ARC for commands and status. The backbone's electrical isolation keeps its return separate from the track supply return. Board and district lights show status locally; a temperature sensor and fan support operation under load.

[Board design](board.kdl)
[PCB layout](dst-01x4.kicad_pcb)

## Firmware revisions

The Embassy firmware crate is [`firmware/dst-01`](../../firmware/dst-01), with
the `x4` variant feature and exactly one hardware revision feature:

| Feature | MCU | District control |
| --- | --- | --- |
| `rev1` | STM32G0B1RE | Two TCA9534 I/O expanders at `0x20` and `0x21`, on I²C3 (PC0 SCL / PC1 SDA) |
| `rev2` | STM32C092CB | Direct MCU GPIO, as described in the adjacent KDL sources |

Rev1's expanders carry phase, programming selection, sleep, and fault signals;
its PWM gates and analog sense inputs connect directly to the MCU. Rev1 is still
a firmware build target even though its Stackup design toolchain is retired.
Its reference is `pre-kdl-local`: the preserved firmware pin map is
`firmware/dst-01x4/src/pins/mk1_rev1.rs`, and the hardware source immediately before
commit `b27db1e` describes rev1. The branch tip's hardware source already describes
rev2, so it must not be used as the rev1 pin map.

See [firmware build instructions](../../firmware/README.md).

## District tasks and recovery

Firmware starts one Embassy task per district. Each owns its active configuration,
hardware handle, and recovery state. `Disabled`, `Synced`, and `Programming` select
whether the district is off or takes the backbone or programming stream. All
districts boot disabled. A separate source-readiness interlock must also allow the
selected stream; the DCC receiver and programming generator have not yet been
implemented, so both readiness flags currently stay false and the rails stay off.

The task uses a `DistrictHal` abstraction. Rev1 serializes access to its two
expanders, preserving the other district's output bits; rev2 uses direct GPIO.
The implementations handle the opposite programming-select polarities (rev1
active high, rev2 active low). Both read ADC current sense and active-low fault
signals, and both can inhibit the district through a direct MCU gate without
waiting for I²C. Source/range changes happen with the gate off. Phase is held
normal; autoreversing is not implemented yet.

Short recovery limits average delivered energy with **burst duty limiting**:
short powered probes separated by unpowered intervals, rather than high-frequency
PWM over the DCC signal. The initial policy is:

- A probe is nominally 2 ms on, followed by at least 40 ms off (about 4.8% duty).
- A bad powered sample immediately ends the probe or continuous drive. The off
  interval doubles after each failed probe, up to 10 seconds.
- Ten consecutive clear probes restore continuous drive. Readings taken while
  off never count as clearance. Startup also passes through these probes.
- Five seconds of healthy continuous operation resets the backoff. Configuration
  changes and disabling/re-enabling cannot erase accumulated short backoff.
- Hardware I/O errors latch that district off. An explicit disabled configuration
  clears the latch; a subsequent enabled configuration starts recovery again.

These timings are local firmware policy, with nominal retry duty at or below
10%. The backoff follows the approach in CommandStation-EX's
`MotorDriver::checkPowerOverload`; its fault-ignore delays and brake PWM are not
ported. The first millisecond of each probe allows driver wake-up and sense-filter
settling. Software trips at 90% of the nominal analog trip-reference voltage:
ADC counts 3334 on rev1 and 2780 on rev2, for either current range. These are
initial engineering thresholds, not calibrated current measurements.

The tasks poll at approximately 1 ms intervals. Shared-bus transactions and
scheduling add latency, so probe durations and duty ratios are nominal, not
hardware-enforced bounds. The DRV8874's local current limiting remains necessary.
Bench validation of thresholds, wake-up timing, short energy, and recovery with
real loads is still required. TIM3 currently supplies Embassy time and must be
reassigned before using it for rev2's pixel stream. The [district configuration
CAN message](../../PROTOCOL.md#district-control-message-0x01-configuration) and
periodic and state-change [status message](../../PROTOCOL.md#district-status-message-0x01-status)
are wired to FDCAN1 on both revisions and exercised in the virtual CAN lab.
Hardware CAN validation, calibrated current measurement,
DCC source readiness, programming generation, and autoreversing remain pending.
