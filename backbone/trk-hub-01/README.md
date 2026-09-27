# ARC-TRK-HUB-01

ARC-TRK-HUB-01 connects the ARC backbone to trackside devices. It receives commands over CAN and manages three separate LIN runs. Each run can serve signal and accessory nodes along the track and can be switched independently.

The hub has its own connection to the layout's supply. It measures the power sent down its LIN runs, so the controller can see their combined load. A status light and an expansion port are available at the hub.

The CAN backbone crosses an electrical isolation boundary here. The shorter LIN runs share the hub's local power and return.

[Board design](board.kdl)
[PCB layout](trk-hub-01.kicad_pcb)
