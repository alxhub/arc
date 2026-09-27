# ARC-TRK-NFC-01

ARC-TRK-NFC-01 is a trackside tag reader. It connects to an ARC track hub over a LIN cable and reads NFC tags through a separate antenna flex. The hub can use a tag detection to identify a train or trigger a layout action.

The board powers its reader locally from the track supply. Its controller and reader interface run at the same logic voltage. The cable's 5 V supply continues through the node for other devices on the run.

[Board design](board.kdl)
[PCB layout](trk-nfc-01.kicad_pcb)
