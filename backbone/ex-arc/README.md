# ARC-EX-ARC

ARC-EX-ARC fits between a CommandStation-EX CSB1 and its motor shield. It lets the command station join ARC's CAN backbone and share its DCC timing with other districts. When the CSB1 acts as a booster, the board can feed it the backbone's DCC timing instead.

A small controller handles backbone communication, selects the booster input, and watches the motor shield's fault outputs. It can brake either track channel locally and tell the CSB1 what happened over a dedicated UART.

[Board design](board.kdl)
[PCB layout](ex-arc.kicad_pcb)
