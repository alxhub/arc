# ARC-DST-01x4 rev 2

ARC-DST-01x4 drives four track districts. It receives the shared DCC timing signal from the ARC backbone, then lets the controller set each district's phase and turn each output on or off independently. A district can also switch to a separate programming signal with a lower current limit.

The board has its own connection to the layout's track supply. It reports total track current and supply voltage to the controller, while each district reports its own current and fault state. That lets the system identify and respond to a problem in one section without treating all four sections as one output.

CAN connects the board to the rest of ARC for commands and status. The backbone's electrical isolation keeps its return separate from the track supply return. Board and district lights show status locally; a temperature sensor and fan support operation under load.

[Board design](board.kdl)
[PCB layout](dst-01x4.kicad_pcb)
