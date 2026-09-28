# ARC firmware

Rust 2024 with Embassy. Run the commands below from this directory; Cargo loads
the aliases and linker settings from `.cargo/config.toml` here.

- `shared/link` (`link`): `no_std` backbone CAN protocol crate, shared by boards
  and host tools. The [protocol specification](../PROTOCOL.md) defines identity
  and Presence; the identity derivation, Presence, district configuration, and
  district status codecs are implemented.
- `dst-01`: district-board firmware. The only variant is `x4`; select exactly one
  of `rev1` and `rev2`. The orthogonal `host` feature builds the same firmware
  crate as a host process for the [virtual CAN lab](../sim/README.md). There is
  no default hardware selection.
- `psu-01`: STM32C092 firmware for automatic ARC-Link power startup, supervised
  INA226 measurements, DCC idle and 128-step locomotive packets, a bounded
  locomotive table, and CAN Presence/status.

```sh
cargo build-dst-rev1 --release
cargo build-dst-rev2 --release
cargo build-psu
cargo test -p link
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

`dst-01` now starts four district tasks behind a revision-specific HAL. Its library
contains the hardware-independent configuration and recovery controller, tested on
the host with `cargo test -p dst-01 --lib`. The binary supplies rev1 expander/GPIO
or rev2 GPIO adapters and shared ADC access. Both builds initialize FDCAN1,
publish Presence and district status, and accept targeted district commands.
The CAN pins and interrupts are revision-specific; frame handling and status
scheduling live in `dst-01/src/network.rs`. The firmware still uses the internal
default clock. Neither revision has been tested on hardware.

The `dst-01-host` binary runs the same district controller with host-side board
models selected by `rev1` or `rev2`, publishes Presence and periodic or state-change status, and
receives district configuration over the simulated CAN bus. All outputs start
disabled. The `host`
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
The shared task operates only through `district::DistrictHal`, leaving revision
details in `dst-01/src/hal/rev1.rs` and `rev2.rs`.

`psu-01` uses the board's 40 MHz external oscillator, TIM3 on PB4 for the DCC
waveform, and an INA226 on I2C1 to qualify the switched ARC-Link rail.
Firmware raises LINK_EN after the monitor responds and enables the DCC and CAN
transceivers after a 20 ms supervised link trial in the 12–18 V range and at
no more than 2 A. A bad measurement latches the link off until reset. The
16-entry locomotive table accepts complete throttle commands, emits immediate
and periodic CAN throttle status, and cycles 128-step DCC packets with an idle
packet between rounds. The host and board use the same table and packet rules.
Command sessions scope sequence numbers but do not grant authority. Authority
renewal, expiry last will, and sender authentication remain to be designed.
This image has been compiled but not tested on a real board.
The `psu-01-host` binary runs the same power controller and packet source
in the [simulator](../sim/README.md). Its signal bus emits each DCC packet's
bits and half-bit durations; district host binaries validate that stream to
decide when their synchronized source is ready.

See the [board README](../backbone/dst-01x4/README.md) for revision hardware and
source references. The shared protocol crate deliberately has no STM32 or
executor dependency.
