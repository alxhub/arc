# ARC-DST-01x6

ARC-DST-01x6 drives six independent track districts from the layout's track supply. It receives DCC timing over the ARC backbone, while CAN carries commands and reports each district's state.

Each district can drive normal train traffic or a programming track at a lower current limit. The board measures its total track current and each district's current, so ARC can identify an overloaded section. Two temperature sensors and a controlled fan support sustained operation.

The six district board serves larger sections of a layout with one controller and one backbone connection. Its link is electrically isolated from the track supply, and its DC input protects against a reversed field connection.

[Board design](board.kdl)
[PCB layout](dst-01x6.kicad_pcb)
