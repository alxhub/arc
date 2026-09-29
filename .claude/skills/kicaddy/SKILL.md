---
name: kicaddy
description: Interrogate this project's KiCAD schematics with the kicaddy CLI. Use whenever you need to know what's on the arc_hat board — components, pins, nets, which pins connect to what, part/symbol details — instead of hand-parsing the `.kicad_sch` S-expression files. Triggers on questions about the schematic, a component, a net, a GPIO connection, or a KiCAD symbol.
---

# Interrogating the schematic with kicaddy

The `.kicad_sch` files in this repo are S-expression documents that are painful to
read by hand. `kicaddy` parses them and answers questions about components, pins,
and nets. **Reach for the CLI before grepping the raw `.kicad_sch`.**

## The binary

```
BIN=~/dev/kicaddy/target/release/kicaddy
```

Not on `$PATH`. If it's missing, build it once: `cd ~/dev/kicaddy && cargo build --release`
(small workspace, ~30s, no jobs cap needed). Source lives at `~/dev/kicaddy`.

## The schematics in this repo (`arc_hat/`)

Hierarchical design — the top sheet references sub-sheets, and **each `.kicad_sch`
is interrogated separately**. There is no combined view; to understand the whole
board, outline each sheet.

- `arc_hat/arc_hat.kicad_sch` — top sheet: GPIO header (J1), EEPROM (U1), ID/WP
  jumper. References the `Core` sub-sheet.
- `arc_hat/core.kicad_sch` — the RP2350A MCU (U2) and its support circuitry.

There is **no `.yaml` schematic** in this repo, so the YAML-only commands below
(`netlist`, `bom`, `compile`, `print-layout`) do not apply here — they operate on
kicaddy's YAML source format, not on `.kicad_sch`. Use `outline` for netlist-style
questions instead.

## Commands for interrogating a `.kicad_sch`

### `outline <file.kicad_sch>` — the workhorse

Semantic summary: every component (library symbol, instances/refdes, pins, human
description), the sub-sheets it references, and the connection list. The connection
list is netlist-like — each line is one net and the pins on it. Nets are prefixed:

- `&NAME` — power/ground net (`&+3V3`, `&GND`, `&+5V`)
- `!NAME` — a named local/global label net (`!ID_SCL`, `!ID_SDA`)
- `$PULLUP(R2, 3.9k, &+3V3)` / `$PULLDOWN(...)` / `$CAPACITOR(C1, 100n, &GND)` —
  kicaddy recognized the passive's role (pull resistor to a rail, decoupling cap)
  and folded it into the net line.

```
$BIN outline arc_hat/arc_hat.kicad_sch     # top sheet: J1 / U1 / jumper + nets
$BIN outline arc_hat/core.kicad_sch        # MCU sheet: U2 (RP2350A) + nets
```

To answer "what connects to U1:SCL" or "which pins are on +3V3", read the
Connections block of the relevant sheet's outline.

### `dump-schematic <file.kicad_sch>` — round-trip / raw fallback

Parses and re-serializes the schematic. Use only when `outline` doesn't expose the
detail you need (exact coordinates, raw properties) and you'd otherwise hand-parse
the file.

## Exploring KiCAD symbol libraries (installed, not this repo)

For "what does this part's pinout look like" or "is there a symbol for X":

```
$BIN symbol-info <Library> <Symbol>    # e.g. symbol-info Device R  → pins + description
$BIN search "<query>" [-n N] [-j]      # e.g. search "i2c eeprom" -n 5   (fuzzy, -j = JSON)
$BIN list-libraries [-v]               # installed symbol libraries (-v adds counts, slow)
$BIN list-symbols <path.kicad_sym> [-v]
$BIN config                            # show detected KiCAD install paths
```

`search` uses a prebuilt index that can go stale (miss parts that exist in the
libs). If a search returns nothing for a part you know exists, rebuild it with
`$BIN index`, then re-search. As a fallback, grep the library directly, e.g.
`grep -o 'symbol "W25Q[0-9A-Za-z_-]*"' "$(dirname "$($BIN config | awk '/Symbols:/{print $2}')")"/symbols/Memory_Flash.kicad_sym`.

## Notes

- `outline` is read-only and safe. The editing commands (`place-component`,
  `add-wire`, `add-label`, `update-component`, `delete-*`, `new-schematic`) mutate
  the `.kicad_sch` in place — don't run them against this repo's schematics unless
  the task is explicitly to edit the board.
- `kicaddy --help` lists the full command set.
