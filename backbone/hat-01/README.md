# ARC-HAT-01

ARC-HAT-01 connects a Raspberry Pi to an ARC layout. It powers the Pi from the cabinet's control supply, gives Linux a CAN interface to the backbone, and lets the Pi listen to DCC timing. The Pi controls the layout's master contactor and reads its auxiliary contact to learn whether the contactor closed.

The HAT also exposes the Pi's I²C bus on a Qwiic connector. It keeps the backbone electrically isolated from the Pi and cabinet control wiring.

[Board design](board.kdl)
[PCB layout](hat-01.kicad_pcb)
