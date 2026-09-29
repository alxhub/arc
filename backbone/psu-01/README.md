# ARC-PSU-01

ARC-PSU-01 supplies the ARC backbone. It takes power from the layout's track supply and switches it onto the cable that serves the other ARC nodes. The switch is off during reset. Firmware closes it automatically after the INA226 monitor responds, then checks the downstream voltage and current before enabling CAN. The DCC line driver also requires a CAN transmit grant. A failed reading or out-of-range voltage/current latches the link off until reset. No CAN command is needed to power the link.

This board is also the usual source of the shared DCC timing signal. It sends that signal to track drivers while CAN carries commands and status between boards. It monitors the power sent onto the backbone and has a light bar for local status.

The firmware repeats a DCC idle packet (`FF 00 FF`) once link power is qualified and layoutd grants transmit permission. Its TIM3 output is driven at 58 µs per half-bit for a `1` and 100 µs per half-bit for a `0`; the line driver stays disabled until both the link reading and transmit permission allow it. A bounded 16-locomotive table cycles 128-step speed packets with an idle packet between rounds and reports each applied state on CAN when it changes and periodically. The board and host simulator both accept valid locomotive throttle frames and send their DCC packets. Command sessions scope sequence numbers; the shared DCC transmit grant persists until MCU restart, without renewal or revocation. DCC readback, console diagnostics, and the light bar also remain to be implemented. DCC timing and fault response still need board-level measurement.

The supply node establishes the backbone's ground reference. Other ARC backbone nodes connect through isolation. A separate USB console provides local diagnostics and setup.

[Board design](board.kdl)
[PCB layout](psu-01.kicad_pcb)
