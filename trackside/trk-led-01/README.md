# ARC-TRK-LED-01

ARC-TRK-LED-01 is a trackside signal node. It connects to a LIN run from an ARC track hub and controls up to twelve signal lamps, arranged as four groups of three. A local pixel output can add an illuminated detail or indicator at the signal.

The hub sends aspect commands; this board turns them into lamp brightness. It reads the supply arriving on the cable so the lamps can keep a consistent appearance when that voltage varies. On startup or after lost communication, the lamps remain dark until the hub commands a state.

It receives its power and communication through the LIN cable. The board also offers a small expansion port for a nearby accessory.

[Board design](board.kdl)
[PCB layout](trk-led-01.kicad_pcb)
