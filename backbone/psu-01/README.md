# ARC-PSU-01

ARC-PSU-01 supplies the ARC backbone. It takes power from the layout's track supply and switches it onto the cable that serves the other ARC nodes. The switch starts off, so the controller can decide when to energize the network.

This board is also the usual source of the shared DCC timing signal. It sends that signal to track drivers while CAN carries commands and status between boards. It monitors the power sent onto the backbone and has a light bar for local status.

The supply node establishes the backbone's ground reference. Other ARC backbone nodes connect through isolation. A separate USB console provides local diagnostics and setup.

[Board design](board.kdl)
[PCB layout](psu-01.kicad_pcb)
