# ARC-TRK-TBL-01xK

ARC-TRK-TBL-01xK is the trackside controller for a Kato turntable. It connects to an ARC track hub over a LIN cable, moves the bridge, operates its lock, and counts the turntable's index contact as the bridge passes each stop. A homing sensor port can establish an absolute starting position.

The rails on the moving bridge receive power from a separate ARC district. This board passes that pair to the Kato cable and reports whether the feed is active. The controller does not drive the track rails.

An expansion port can host a nearby angle sensor or other I²C accessory.

[Board design](board.kdl)
[PCB layout](trk-tbl-01xK.kicad_pcb)
