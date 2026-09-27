# ARC-TRK-TRN-01

ARC-TRK-TRN-01 is a trackside turnout controller. It connects to a LIN leg and operates four turnout machines, two on each terminal bank. A local energy store supplies the brief throw pulses so they do not pull that current through the leg cable.

The node measures the store before and after a throw, reports machine faults to the hub, and shows when it is ready to operate. A Qwiic port provides local expansion for trackside accessories.

[Board design](board.kdl)
[PCB layout](trk-trn-01.kicad_pcb)
