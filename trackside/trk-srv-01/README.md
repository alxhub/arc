# ARC-TRK-SRV-01

ARC-TRK-SRV-01 is a trackside node for moving and sounding scenery. It receives commands from an ARC track hub over a LIN cable and controls eight servos, a speaker, and one three lamp signal head. A pixel output can add local lighting.

The node makes its own 5 V supply for the servos, speaker, and pixels. The cable also carries 5 V for other nodes on the same run; this board passes that supply through without using it. Its servo outputs and speaker stay inactive during startup until the controller enables them.

A small expansion port can power a nearby I²C accessory.

[Board design](board.kdl)
[PCB layout](trk-srv-01.kicad_pcb)
