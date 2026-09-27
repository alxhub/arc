# ARC KDL designs

ARC is a distributed control system for a model railroad. It puts control close to the track, while a shared backbone coordinates power, commands, status, and DCC timing. A fault in one track district can be measured and handled independently of the others; turnout, signal, and scenery controllers can sit where their wires are shortest.

## How ARC is arranged

The **ARC Link backbone** carries a switched supply, CAN communication, and a differential DCC timing pair. The supply board establishes the link's ground reference and normally sources the DCC signal. Other active backbone boards isolate their local circuits from the link. Track district boards use the common timing signal to generate their own rail power, with separate control and fault sensing for each district. The DCC pair carries timing, not the traction current delivered to the rails.

A **trackside hub is a backbone board**. It receives CAN commands and fans them out over three shorter LIN runs. Trackside boards use those runs for local accessories such as lamps, turnouts, servos, and tag readers. A separate antenna flex connects to the NFC reader. This split keeps the backbone focused on layout wide coordination and puts accessory wiring near the devices it serves.

The boards are composed from reusable KDL circuits in [`shared/`](shared/). Each board directory keeps its `board.kdl`, supporting KDL files, functional README, and KiCad PCB project together.

## Backbone boards

| Board | Role |
| --- | --- |
| [ARC-PSU-01](backbone/psu-01/README.md) | Switches power onto ARC Link and normally generates DCC timing. |
| [ARC-HAT-01](backbone/hat-01/README.md) | Connects a Raspberry Pi to CAN and controls the layout contactor. |
| [ARC-EX-ARC](backbone/ex-arc/README.md) | Connects a CommandStation-EX and its motor shield to ARC. |
| [ARC-TERM-01](backbone/term-01/README.md) | Terminates the CAN and DCC pairs at the end of a run. |
| [ARC-DST-01x4 rev 2](backbone/dst-01x4/README.md) | Drives four independent track districts. |
| [ARC-DST-01x6](backbone/dst-01x6/README.md) | Drives six independent track districts. |
| [ARC-DST-01x8](backbone/dst-01x8/README.md) | Drives eight independent track districts. |
| [ARC-YRD-01x8](backbone/yrd-01x8/README.md) | Controls yard turnouts and signal lamps. |
| [ARC-TRK-HUB-01](backbone/trk-hub-01/README.md) | Bridges backbone CAN to three trackside LIN runs. |

## Trackside boards

| Board | Role |
| --- | --- |
| [ARC-TRK-LED-01](trackside/trk-led-01/README.md) | Drives signal lamps on a LIN run. |
| [ARC-TRK-SRV-01](trackside/trk-srv-01/README.md) | Drives servos, sound, and a signal head. |
| [ARC-TRK-NFC-01](trackside/trk-nfc-01/README.md) | Reads vehicle tags with a separate antenna. |
| [ARC-TRK-ANT-01](trackside/trk-ant-01/README.md) | Provides the reader's flexible track antenna. |
| [ARC-TRK-TRN-01](trackside/trk-trn-01/README.md) | Operates four turnout machines. |
| [ARC-TRK-TBL-01xK](trackside/trk-tbl-01xK/README.md) | Operates a Kato turntable. |

## Working with the designs

The KDL imports use the [stackup parts library](https://github.com/stackup-eda/library), pinned in `manifest.kdl`. For local development, add a `manifest.local.kdl` with a path to a parts checkout:

```kdl
library stackup path="../stackup-parts"
```

Run `stackup check backbone/psu-01/board.kdl` to validate a board. Substitute any other board's `board.kdl` path as needed. Use `--locked` to check against the pinned library commit. Without a local override, the CLI fetches Git libraries into `.stackup/cache/` beside `manifest.kdl`.

Each KiCad PCB has a `.stackup_sch` sidecar pointing to the adjacent `board.kdl`. To sync a closed board with the stackup KDL KiCad plugin, using KiCad's bundled Python (`STACKUP_KDL` is the path to that worktree):

```sh
/Applications/KiCad/KiCad.app/Contents/Frameworks/Python.framework/Versions/3.9/bin/python3 \
  "$STACKUP_KDL/kicad-plugin/sync_headless.py" \
  backbone/term-01/term-01.kicad_pcb
```

The PCB layouts retain their routed copper and KiCad project settings. Some newer KDL parts on the district and yard boards still need placement and routing. A KDL check or PCB sync is not a DRC or fabrication signoff. The antenna board retains an older footprint table as `fp-lib-table.legacy` for reference; KiCad uses its existing `fp-lib-table`.
