# ARC firmware

Rust 2024 with Embassy. Run the commands below from this directory; Cargo loads
the aliases and linker settings from `.cargo/config.toml` here.

- `shared/link` (`link`): `no_std` backbone CAN protocol crate, shared by boards
  and host tools. The [protocol specification](../PROTOCOL.md) defines identity
  and Presence; the identity derivation and Presence codec are implemented.
- `dst-01`: district-board firmware. The only variant is `x4`; select exactly one
  of `rev1` and `rev2`. The orthogonal `host` feature builds the same firmware
  crate as a host process for the [virtual CAN lab](../sim/README.md). There is
  no default hardware selection.

```sh
cargo build-dst-rev1 --release
cargo build-dst-rev2 --release
cargo test -p link
cargo test -p dst-01 --lib
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
or rev2 GPIO adapters and shared ADC access. It still uses the internal default
clock; CAN clock setup and protocol messages are pending. Neither revision has
been tested on hardware.

The `dst-01-host` binary runs the same district controller with host-side board
models selected by `rev1` or `rev2`, and publishes Presence on the simulated CAN
bus. The `host` feature does not select a revision; build it with `x4,host,rev1`
or `x4,host,rev2`. See the [simulation lab](../sim/README.md) for the component
model and its current limits.

On embedded targets, callers submit a complete validated configuration with
`dst_01::tasks::DISTRICTS[index].configure(config)` and read its applied state with
`status()`. Configuration updates use a latest-value mailbox. The DCC receiver and
programming generator will publish `tasks::Sources` through `tasks::SOURCES`;
neither source is ready at startup, so configuration alone cannot energize rails.
The shared task operates only through `district::DistrictHal`, leaving revision
details in `dst-01/src/hal/rev1.rs` and `rev2.rs`.

See the [board README](../backbone/dst-01x4/README.md) for revision hardware and
source references. The shared protocol crate deliberately has no STM32 or
executor dependency.
