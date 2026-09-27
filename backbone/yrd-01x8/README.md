# ARC-YRD-01x8

ARC-YRD-01x8 controls eight turnout machines and sixteen signal lamps from one ARC node. CAN carries commands over the backbone. The board distributes those commands to eight independent machine outputs and an I²C lamp driver.

Each machine channel can throw in either direction and reports its current to the controller. The board sets a shared current limit for the active throw and measures its supply voltage. It supports reversible solenoids and stall motors; a dual-coil machine’s common connects to the cabinet supply return.

The signal header provides sixteen individually dimmable outputs for common-anode lamps. A Qwiic port shares its I²C bus for local accessories. Board status is visible on an RGB lamp, while the separate regulator lamp shows that logic power is present.

[Board design](board.kdl)
[PCB layout](yrd-01x8.kicad_pcb)
