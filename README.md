# ARC — model railroad control

ARC is my hobby ecosystem of model railroad control systems. It is designed as an integrated, full stack system, from the computer running the layout to the boards powering track districts, moving turnouts, and lighting signal masts.

The design emphasizes global processing and specialized boards for dedicated tasks. A layout computer knows the track plan and coordinates the whole railroad, while distributed hardware provides the power, sensing, and local control each task needs. This gives the software a layout-wide view while keeping physical connections close to the equipment they serve.

ARC is designed for evolving and temporary layouts, with plug-and-play operation as a guiding goal. Common capabilities are available throughout the system, so changing the track plan can be an ordinary part of using the railroad. That flexibility goes hand in hand with intelligent control: the computer can use its knowledge of track connections and turnout positions to prepare the hardware for a route.

This repository contains ARC's schematics written in [Stackup](https://github.com/stackup-eda/stackup) and the corresponding KiCad PCB projects. Stackup is another of my projects: a declarative language and toolchain for describing electronic circuits as text. It expresses parts, reusable circuits, connections, and design constraints, checks them together, and exports the result for KiCad. Stackup uses KDL as its file syntax, which is why the schematic sources have a `.kdl` extension.

The architecture and intended system behavior are described below; individual board READMEs cover their roles and design decisions.

## Three levels of control

ARC organizes the layout into three levels:

1. **The layout computer**, usually a Raspberry Pi, knows the track plan, runs the layout, and coordinates its behavior. The ARC HAT connects the Pi to the backbone and provides control of the layout's master contactor.
2. **The ARC-Link backbone** connects primary functional boards, including DCC district drivers and Trackside Hubs, over CAN. These boards typically live together in one or more clusters around the layout and are DIN-rail mountable.
3. **Trackside bus legs** extend from those clusters along the rails. They connect boards that serve a local purpose, such as a signal mast, turnout, servo, or vehicle tag reader.

A **Trackside Hub is a backbone board** that connects these last two levels. It receives commands over ARC-Link and manages up to three separate trackside bus legs, using LIN for local signaling. This arrangement lets the backbone connect clusters of functional hardware while the trackside wiring follows the railroad to individual accessories.

## Power and installation

ARC is designed for industrial DC power supplies: **12 V for N scale** and **15 V for HO scale**, with a particular emphasis on DIN-rail mounted supplies. Primary backbone boards share that mounting approach, making it practical to assemble a cluster of power and control hardware together.

The power architecture distinguishes the supply carried by ARC-Link from the power delivered to the rails and accessories. The supply board switches power onto ARC-Link; district drivers and Trackside Hubs also have their own connections to the layout's supply for their loads. District drivers generate rail power locally from the shared DCC timing signal.

## ARC-Link: the backbone

ARC-Link carries power, CAN communication, and DCC synchronization over **eight-wire Ethernet cable**:

| Conductors | Purpose |
| --- | --- |
| 2 | Backbone power |
| 2 | Backbone ground |
| 2 | CAN differential pair for commands and status |
| 2 | DCC differential pair for synchronized track timing |

Ethernet cable is the wiring medium; ARC-Link uses CAN for communication. The separate DCC pair carries timing, not the traction current delivered to the rails. The supply board normally generates that timing, and district drivers use it to produce their individual DCC outputs.

**Ground loop isolation is a central design priority.** The supply board establishes the backbone's ground reference, and other active backbone boards isolate their local circuits from the link. This separates the backbone reference from local track supply returns and control wiring as the system extends between clusters around the layout.

ARC-Link is distinct from LCC. Support for LCC devices is planned through a dedicated **ARC ↔ LCC bridge**, allowing those devices to participate in the layout alongside ARC hardware.

## The trackside bus

The trackside bus uses **four-wire telephone cable with RJ-11 connectors**. Each leg carries four connections:

| Conductor | Purpose |
| --- | --- |
| Track voltage | The layout's track-voltage supply for local loads |
| 5 V | Low-voltage power |
| Ground | Local power and signaling return |
| LIN | Commands and status for trackside boards |

These legs typically parallel the rails, keeping accessory wiring near the devices it serves. A signal controller can sit near its mast, a turnout controller near its turnout machines, and a tag reader near its separate antenna flex.

Each Trackside Hub manages up to three legs and can switch them independently. Its backbone connection crosses an isolation boundary; the trackside legs share the hub's local power and return.

## A dedicated output for every district

ARC calls a track block a **district** and standardizes on a distinct, independently controlled power feed for each one. The system is optimized for many separate outputs, with district boards providing four, six, or eight outputs per board. This makes each district a unit of power control, sensing, and fault handling.

Every district is designed to include the same core capabilities:

- **Built-in block detection through drive-current sensing.** This detects locomotives, but does not detect resistive wheelsets.
- **Built-in autoreversing.** Each district can reverse its output phase as needed for reversing track arrangements, including advance changes coordinated by the layout computer.
- **Programming-track operation on demand.** Any district can serve as a fully featured programming track when needed, with a separate programming signal and a lower current limit.
- **Independent control and fault reporting.** Each output can be switched and monitored separately, so a fault can be identified and handled at the affected district.

These capabilities are part of the district design rather than separate accessories to add to selected sections. A district's role can change with the layout's needs, including switching between normal running and programming operation.

## Layouts that evolve, control that understands them

Having autoreversing available in every district makes it easier to experiment with the track plan. Add a reversing loop on a whim, and the surrounding districts already have the phase control they need: there is no need to carefully rewire them around a specially designated reversing output. Dedicated district feeds and a controller that knows the revised track plan provide the basis for this plug-and-play approach.

That global picture also enables proactive polarity control. ARC is designed to use the track plan and turnout configurations to align district polarities automatically before wheels cross a boundary and cause a short. The layout computer can coordinate the outputs involved in a route because it knows how those sections connect.

The Kato turntable controller is another example of this intended behavior. As the turntable spins, ARC can use its knowledge of the table's position and connecting tracks to align their polarities automatically. The specialized board operates the turntable, while the layout-wide picture makes coordinated track power possible.

## Backbone boards

| Board | Role |
| --- | --- |
| [ARC-PSU-01](backbone/psu-01/README.md) | Switches power onto ARC-Link and normally generates DCC timing. |
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

The boards are composed from reusable Stackup circuits in [`shared/`](shared/). Each board directory keeps its `board.kdl`, supporting Stackup sources, functional README, and KiCad PCB project together.

The designs import the [Stackup parts library](https://github.com/stackup-eda/library), pinned in `manifest.kdl`. For local development, add a `manifest.local.kdl` with a path to a parts checkout:

```kdl
library stackup path="../stackup-parts"
```

Run `stackup check backbone/psu-01/board.kdl` to validate a board. Substitute any other board's `board.kdl` path as needed. Use `--locked` to check against the pinned library commit. Without a local override, the CLI fetches Git libraries into `.stackup/cache/` beside `manifest.kdl`.

Each KiCad PCB has a `.stackup_sch` sidecar pointing to the adjacent `board.kdl`. To sync a closed board with the Stackup KiCad plugin, using KiCad's bundled Python (`STACKUP_KDL` is the path to the Stackup worktree):

```sh
/Applications/KiCad/KiCad.app/Contents/Frameworks/Python.framework/Versions/3.9/bin/python3 \
  "$STACKUP_KDL/kicad-plugin/sync_headless.py" \
  backbone/term-01/term-01.kicad_pcb
```

The PCB layouts retain their routed copper and KiCad project settings. Some newer parts in the district and yard schematics still need placement and routing. A Stackup check or PCB sync is not a DRC or fabrication signoff. The antenna board retains an older footprint table as `fp-lib-table.legacy` for reference; KiCad uses its existing `fp-lib-table`.
