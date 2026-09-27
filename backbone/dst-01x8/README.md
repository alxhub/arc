# ARC-DST-01x8

ARC-DST-01x8 drives eight independent track districts from the layout's track supply. It receives the shared DCC timing signal over the ARC backbone and uses CAN for commands and status.

Each district can switch between running trains and a lower current programming mode. The controller receives a separate fault and current measurement from every district, plus a measurement of total track current. Two temperature sensors watch the board's two groups of four districts.

This board lets a larger area of a layout share one controller and backbone connection while keeping its track outputs independent. Its backbone connection is electrically isolated from the track supply, and its DC input protects against a reversed field connection.

[Board design](board.kdl)
[PCB layout](dst-01x8.kicad_pcb)
