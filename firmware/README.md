# ARC firmware

Rust 2024 with Embassy. Run the commands below from this directory; Cargo loads
the aliases and linker settings from `.cargo/config.toml` here.

- `shared/link` (`link`): `no_std` backbone CAN protocol crate, shared by boards
  and host tools. The [protocol specification](../PROTOCOL.md) defines identity
  and Presence; the identity derivation, Presence, district configuration, and
  district status codecs are implemented.
- `shared/dcc` (`dcc`): `no_std` packet encoder, throttle table, continuous bit
  engine, and optional STM32 timer transmitter shared by PSU and DST rev1.
- `dst-01`: district-board firmware. The only variant is `x4`; select exactly one
  of `rev1` and `rev2`. The orthogonal `host` feature builds the same firmware
  crate as a host process for the [virtual CAN lab](../sim/README.md). There is
  no default hardware selection.
- `psu-01`: STM32C092 firmware for automatic ARC-Link power startup, supervised
  INA226 measurements, DCC idle and 128-step locomotive packets, a bounded
  locomotive table, and CAN Presence/status.
- `arc-boot`: Embassy bootloader for the G0B1 and C092 application partitions.

```sh
cargo build-dst-rev1 --release
cargo build-dst-rev2 --release
cargo build-psu
cargo build-boot-g0b1
cargo build-boot-c092
cargo test -p link
cargo test -p dcc
cargo test -p dst-01 --lib
cargo test -p psu-01 --lib
cargo fmt --all --check
```

The build aliases expand to `cargo build -p dst-01 --target thumbv6m-none-eabi
--no-default-features --features x4,rev1` (or `x4,rev2`). Both MCUs use this target;
the revision feature selects the Embassy chip and its generated linker memory
map. Build revisions separately: `--all-features` cannot represent one board.
Each build writes `target/thumbv6m-none-eabi/release/dst-01`, replacing the previous
revision's artifact. Use `--target-dir target/rev1` or `target/rev2` to retain both.

The workspace default member is `link`, so plain `cargo build` and `cargo test`
work on the host. The toolchain file pins Rust and installs the embedded target.
Commit `Cargo.lock` to keep firmware dependencies reproducible.

### CAN firmware updates

All three embedded targets accept the same [CAN FD firmware update
protocol](../PROTOCOL.md#firmware-update-over-can-fd). It uses Embassy's
`embassy-boot-stm32` bootloader and blocking firmware updater. Bootloader and
application builds now have separate flash regions:

| MCU | Bootloader | Active application | Staging | Boot state |
| --- | --- | --- | --- | --- |
| STM32G0B1RE (DST rev1) | `0x08000000`, 16 KiB | `0x08004000`, 240 KiB | `0x08040000`, 248 KiB | `0x0807e000`, 8 KiB |
| STM32C092CB (DST rev2, PSU) | `0x08000000`, 16 KiB | `0x08004000`, 54 KiB | `0x08011800`, 56 KiB | `0x0801f800`, 2 KiB |

Provision each board once with the appropriate `arc-boot` image at the start
of flash and its board application at `0x08004000`. Existing applications linked
at `0x08000000` need this initial reflash through SWD or the available ROM USB
DFU route. The C092 bootloader binary is shared by DST rev2 and PSU. Keep
bootloader and application build artifacts separate with `--target-dir`, as
revision builds otherwise overwrite the previous artifact. Convert the
application ELF to a raw `.bin` with `llvm-objcopy -O binary` before transfer.
Rev1 reserves the last 16 bytes of RAM for the USB console's ROM bootloader
request; the ARC bootloader consumes that marker before starting the application.
The linker partition symbols are offsets from the start of flash, as required
by Embassy's flash driver; the physical image addresses in the table remain
absolute. The initial-image packager checks those symbols in both ELFs and
rejects an image built with the earlier absolute-address layout.

#### Initial rev1 USB provisioning

A factory-blank G0B1 enters the STM32 system-memory loader through its empty
flash check. With the board powered and USB-C connected to a data-capable host,
use STM32CubeProgrammer's USB/DFU connection. Build and package both images from
this directory:

```sh
cargo build-boot-g0b1 --target-dir target/boot-g0b1
cargo build-dst-rev1 --release --target-dir target/dst-rev1
python3 tools/prepare_initial_rev1.py \
  target/boot-g0b1/thumbv6m-none-eabi/release/arc-boot \
  target/dst-rev1/thumbv6m-none-eabi/release/dst-01 \
  target/provision-rev1
```

Program `target/provision-rev1/dst-rev1-initial.bin` as a **binary** at
`0x08000000`, with verify enabled. This single image includes the Embassy
bootloader at `0x08000000` and the rev1 application at `0x08004000`; the staging
and state partitions stay erased. For CubeProgrammer CLI, the equivalent is
`STM32_Programmer_CLI -c port=usb1 -w target/provision-rev1/dst-rev1-initial.bin 0x08000000 -v`.
Power-cycle after a successful verify so the G0B1 rechecks the formerly empty
flash, then look for the ARC USB CDC console. Keep ARC-Link and district loads
disconnected for this first boot. The application ELF has an ELF-header load
segment below `0x08004000`; program the packaged binary, not that ELF directly.
On rev1, the application blinks the RGB status LED through red, green, blue,
yellow, cyan, magenta, and white. The separate green LED on MCU pin 38 (PC6)
blinks with each color; each color and dark interval lasts 300 ms. These LEDs
are an early sign that the application scheduler is running, before checking USB.
For a later application-only flash, enter ROM DFU with `bootloader`, program
`target/provision-rev1/dst-rev1.bin` as a binary at `0x08004000` with verify,
then power-cycle. This preserves the ARC bootloader at `0x08000000`.
On board #2, the RGB green channel was not visibly lighting during initial
bring-up. Its programmed GPIO is PD9 (MCU pin 41), and the rev1 PCB routes it
through R40 (300 Ω) to D3's green cathode. R40's MCU side was observed
alternating between 0 V and 3.3 V with the initial push-pull build. The LED
outputs now use open drain so their off state is high impedance; that change
has not yet been flashed onto board #2. Check R40's LED side and D3 if green
remains dark after the update.
Board #1 was recovered by erasing flash over an ESP32-driven SWD connection,
then reflashed with the open-drain build. Its small green D4 blinks, but RGB D3
is dark; D4 and D3 have separate supply rails, so the RGB lamp rail needs a
voltage check. D3 pad 4 measured about 4.5 V on board #1, so the rail reaches
the LED footprint. A meter-provided ground path at the RGB resistors lit red
and blue but not green; the normal GPIO sink paths have not yet been measured.

On a Linux host with a configured CAN FD SocketCAN interface, run:

```sh
python3 tools/can_update.py --interface can0 --sender-id 000001 \
  --target-id fa5138 --kind dst-rev1 path/to/dst-01.bin
```

Select `dst-rev2` or `psu` for the other applications, and use the target's
actual Presence network ID. The sender ID must be unique on the bus; `000001`
is the virtual lab's configured host identity. The tool checks the image's
stack and reset vectors before sending, then waits for an acknowledgement of
each chunk. The board checks staged flash and lets Embassy swap or roll back
the image. Bench validation of flash swapping and recovery on real hardware
is still required.

`dst-01` now starts four district tasks behind a revision-specific HAL. Its library
contains the hardware-independent configuration and recovery controller, tested on
the host with `cargo test -p dst-01 --lib`. The binary supplies rev1 expander/GPIO
or rev2 GPIO adapters and shared ADC access. Both builds initialize FDCAN1,
publish Presence and district status, and accept targeted district commands.
The CAN pins and interrupts are revision-specific; frame handling and status
scheduling live in `dst-01/src/network.rs`. Rev1 uses its 40 MHz external oscillator for system and CAN timing; rev2
still uses the internal default clock. Rev1 boot, LED heartbeat, and USB CDC
enumeration have been observed on board #2; CAN and district operation still
need hardware validation. Rev2 has not been tested on hardware.

The `dst-01-host` binary runs the same district controller with host-side board
models selected by `rev1` or `rev2`, publishes Presence and periodic or state-change status, and
receives district configuration over the simulated CAN bus. All outputs start
disabled. Its required `--clock` argument connects it to the simulator's
logical world clock; firmware timers do not follow elapsed wall time. The `host`
feature does not select a revision; build it with `x4,host,rev1`
or `x4,host,rev2`. See the [simulation lab](../sim/README.md) for the component
model and its current limits.

On embedded targets, callers submit a complete validated configuration with
`dst_01::tasks::DISTRICTS[index].configure(config)` and read its applied state with
`status()`. CAN commands feed the same latest-value configuration mailbox. The DCC receiver and
programming generator will publish `tasks::Sources` through `tasks::SOURCES`;
neither source is ready at startup, so configuration alone cannot energize rails.
The embedded ADC path currently reports only an overcurrent threshold. Status
frames mark current measurement invalid and set average and peak to zero until
calibrated sampling is implemented.
PSU and DST rev1 use the same `dcc::Permission` latch. A targeted CAN DCC Grant
sets it until MCU restart; duplicate grants are harmless. Both start with DCC
disabled. Power qualification gates actual output without clearing permission.
Rev1 uses TIM14/PC12 and PC13 driver-enable, with active-low link-good on PC2.
CAN capability/permission status lets layoutd choose a master; rev2 reports no
transmitter and ignores grants. Preparing throttle entries never enables DCC.
TX generation does not set receiver readiness or enable any district.

### Rev1 USB debug console

Rev1 exposes a USB CDC serial port on PA11/PA12. Open it with a terminal using
newline-terminated ASCII commands (the baud rate setting is ignored):

| Command | Effect |
| --- | --- |
| `help` | List commands. |
| `led <auto\|off\|red\|green\|blue\|white>` | Resume the RGB blink or hold the selected color for DC measurements. The small green LED follows the selected color. |
| `gpio` | Report the LED pins' GPIO mode, open-drain bit, output sink latch, and sensed input level. A floating off pin's sensed level is not a reliable voltage measurement. |
| `master` | Grant this board DCC transmit permission for this boot. The link-good interlock still gates the driver. |
| `throttle <address> <f\|r> <speed>` | Set a short (1–127) or long (128–10239) locomotive address, forward or reverse, at speed 0–126. |
| `current <ms>` | Log nominal milliamps for all four districts at this interval; `current 0` stops logging. |
| `status` | Show each district's state, trip flag, nominal milliamps, and latest ADC count. State changes are also printed automatically while connected. |
| `bootloader` | Reset directly into the STM32 system-memory USB DFU bootloader for flashing. |
| `erase` | Erase the 2 KiB flash page at `0x08000000` and reset into STM32 ROM USB DFU. This removes the ARC bootloader until it is reflashed. |

The console's local `master` command is for bench debugging and bypasses CAN
master selection; it does not bypass link power qualification. USB descriptors
currently use provisional VID/PID `1209:a0c1`. The ROM bootloader uses its own
USB identity, so the CDC port disappears after `bootloader` and the board must
be flashed through a DFU tool. Rev1 USB CDC, the `bootloader` ROM handoff,
CubeProgrammer DFU detection, and the power-cycle return to the application
have been observed on board #2. The `erase` command has not been validated on
a physical board.
`erase` uses the G0B1 empty-flash boot check. After erasing page 0, firmware
also sets `FLASH_ACR.EMPTY` so a software reset enters ROM. To restore normal
boot, program the ARC bootloader at `0x08000000` and the application at
`0x08004000`, then reload option bytes or power-cycle to update the empty check.
The `bootloader` command leaves flash intact; `erase` is for reinstalling the
bootloader itself. If `BOOT_LOCK` forces boot from main flash, `erase` reports
an error and leaves page 0 intact. The erase path also needs validation on rev1
hardware.

Rev1 current conversion uses the preserved rev1 schematic: 2.21 kΩ at IPROPI
in running mode, 22.1 kΩ in programming mode, no ADC divider, nominal 3.3 V
ADC reference, and the DRV8874's typical 450 µA/A current mirror. The displayed
milliamps are nominal estimates from the latest ADC sample. Mirror error,
resistor tolerance, and ADC reference error remain until bench calibration;
the DRV8874 specifies up to ±30 mA mirror offset below 0.4 A. The console uses
the configured district mode to select the scale.

The shared task operates only through `district::DistrictHal`, leaving revision
details in `dst-01/src/hal/rev1.rs` and `rev2.rs`.

`psu-01` uses the board's 40 MHz external oscillator, TIM3 on PB4 for the DCC
waveform, and an INA226 on I2C1 to qualify the switched ARC-Link rail.
Firmware raises LINK_EN after the monitor responds and enables the CAN
transceiver after a 20 ms supervised link trial in the 12–18 V range and at
no more than 2 A. The DCC driver additionally requires the CAN grant. A bad measurement latches the link off until reset. The
16-entry locomotive table accepts complete throttle commands, emits immediate
and periodic CAN throttle status, and cycles 128-step DCC packets with an idle
packet between rounds. The host and board use the same table and packet rules.
Command sessions scope sequence numbers but do not grant authority. Transmit permission lasts until restart; sender authentication is not implemented.
This image has been compiled but not tested on a real board.
The `psu-01-host` binary runs the same power controller and packet source
in the [simulator](../sim/README.md) and also requires `--clock`. Its signal bus emits each DCC packet's
bits and half-bit durations; district host binaries validate that stream to
decide when their synchronized source is ready.

See the [board README](../backbone/dst-01x4/README.md) for revision hardware and
source references. The shared protocol crate deliberately has no STM32 or
executor dependency.
