//! Host-side board components behind the same district controller contract.

use crate::district::{Drive, Mode, Sample};

#[derive(Clone, Copy)]
struct Drv8874 {
    drive: Drive,
    shorted: bool,
    load_ma: u16,
}

impl Drv8874 {
    fn new() -> Self {
        Self {
            drive: Drive {
                mode: Mode::Disabled,
                enabled: false,
            },
            shorted: false,
            load_ma: 150,
        }
    }

    fn current_ma(&self) -> u16 {
        if !self.drive.enabled {
            0
        } else if self.shorted {
            5_000
        } else {
            self.load_ma
        }
    }

    fn fault(&self) -> bool {
        self.drive.enabled && self.shorted
    }

    fn overcurrent(&self) -> bool {
        self.current_ma() >= 3_000
    }
}

#[cfg(feature = "rev1")]
mod revision {
    use super::{Drive, Mode};

    #[derive(Clone, Copy)]
    struct Tca9534 {
        output: u8,
        direction: u8,
    }

    impl Tca9534 {
        const fn new() -> Self {
            Self {
                output: 0,
                direction: 0xff,
            }
        }

        fn write(&mut self, register: u8, value: u8) {
            match register {
                1 => self.output = value,
                3 => self.direction = value,
                _ => panic!("unsupported TCA9534 register"),
            }
        }

        fn input(&self, even_fault: bool, odd_fault: bool) -> u8 {
            let mut value = 0xff;
            if even_fault {
                value &= !(1 << 2);
            }
            if odd_fault {
                value &= !(1 << 5);
            }
            value
        }
    }

    pub struct BoardIo {
        expanders: [Tca9534; 2],
        gates: [bool; 4],
        initialized: bool,
    }

    impl BoardIo {
        pub fn new() -> Self {
            Self {
                expanders: [Tca9534::new(); 2],
                gates: [false; 4],
                initialized: false,
            }
        }

        fn initialize(&mut self) {
            if !self.initialized {
                for expander in &mut self.expanders {
                    expander.write(1, 0);
                    expander.write(3, 0x24);
                }
                self.initialized = true;
            }
        }

        pub fn apply(&mut self, index: usize, drive: Drive) {
            self.gates[index] = false;
            self.initialize();
            let which = index / 2;
            let (prog, sleep, mask) = if index.is_multiple_of(2) {
                (1, 3, 0x0b)
            } else {
                (6, 4, 0xd0)
            };
            let bits =
                ((drive.mode == Mode::Programming) as u8) << prog | (drive.enabled as u8) << sleep;
            let expander = &mut self.expanders[which];
            expander.write(1, (expander.output & !mask) | bits);
            self.gates[index] = drive.enabled;
        }

        pub fn fault(&self, index: usize, faults: [bool; 4]) -> bool {
            let which = index / 2;
            let input = self.expanders[which].input(faults[which * 2], faults[which * 2 + 1]);
            input & (1 << if index.is_multiple_of(2) { 2 } else { 5 }) == 0
        }

        pub fn inhibit(&mut self, index: usize) {
            self.gates[index] = false;
        }
        pub fn gate(&self, index: usize) -> bool {
            self.gates[index]
        }
    }

    #[cfg(test)]
    impl BoardIo {
        pub fn output(&self, which: usize) -> u8 {
            self.expanders[which].output
        }
        pub fn direction(&self, which: usize) -> u8 {
            self.expanders[which].direction
        }
    }
}

#[cfg(feature = "rev2")]
mod revision {
    use super::{Drive, Mode};

    pub struct BoardIo {
        gates: [bool; 4],
        sleeps: [bool; 4],
        nprogs: [bool; 4],
    }

    impl BoardIo {
        pub fn new() -> Self {
            Self {
                gates: [false; 4],
                sleeps: [false; 4],
                nprogs: [true; 4],
            }
        }

        pub fn apply(&mut self, index: usize, drive: Drive) {
            self.gates[index] = false;
            self.sleeps[index] = false;
            self.nprogs[index] = drive.mode != Mode::Programming;
            if drive.enabled {
                self.sleeps[index] = true;
                self.gates[index] = true;
            }
        }

        pub fn fault(&self, index: usize, faults: [bool; 4]) -> bool {
            faults[index]
        }
        pub fn inhibit(&mut self, index: usize) {
            self.gates[index] = false;
            self.sleeps[index] = false;
        }
        pub fn gate(&self, index: usize) -> bool {
            self.gates[index]
        }

        #[cfg(test)]
        pub fn pin_state(&self, index: usize) -> (bool, bool, bool) {
            (self.gates[index], self.sleeps[index], self.nprogs[index])
        }
    }
}

pub struct Board {
    io: revision::BoardIo,
    drivers: [Drv8874; 4],
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

impl Board {
    pub fn new() -> Self {
        Self {
            io: revision::BoardIo::new(),
            drivers: [Drv8874::new(); 4],
        }
    }

    pub fn inject_short(&mut self, index: usize, shorted: bool) {
        self.drivers[index].shorted = shorted;
    }

    pub fn apply(&mut self, index: usize, drive: Drive) {
        self.io.apply(index, drive);
        self.drivers[index].drive = drive;
    }

    pub fn inhibit(&mut self, index: usize) {
        self.io.inhibit(index);
        self.drivers[index].drive.enabled = false;
    }

    pub fn sample(&self, index: usize) -> Sample {
        let faults = core::array::from_fn(|which| self.drivers[which].fault());
        Sample {
            fault: self.io.fault(index, faults),
            overcurrent: self.drivers[index].overcurrent(),
        }
    }

    pub fn current_ma(&self, index: usize) -> u16 {
        self.drivers[index].current_ma()
    }
    pub fn gate(&self, index: usize) -> bool {
        self.io.gate(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_only_appears_while_driver_is_energized() {
        let mut board = Board::new();
        board.inject_short(0, true);
        assert_eq!(
            board.sample(0),
            Sample {
                fault: false,
                overcurrent: false
            }
        );
        board.apply(
            0,
            Drive {
                mode: Mode::Synced,
                enabled: true,
            },
        );
        assert!(board.gate(0));
        assert_eq!(board.current_ma(0), 5_000);
        assert_eq!(
            board.sample(0),
            Sample {
                fault: true,
                overcurrent: true
            }
        );
        board.inhibit(0);
        assert_eq!(board.current_ma(0), 0);
        assert_eq!(
            board.sample(0),
            Sample {
                fault: false,
                overcurrent: false
            }
        );
    }

    #[cfg(feature = "rev1")]
    #[test]
    fn rev1_uses_expander_outputs_and_fault_inputs() {
        let mut board = Board::new();
        board.apply(
            0,
            Drive {
                mode: Mode::Programming,
                enabled: true,
            },
        );
        assert_eq!(board.io.direction(0), 0x24);
        assert_eq!(board.io.output(0) & 0x0b, 0x0a);
        board.inject_short(0, true);
        assert!(board.sample(0).fault);
    }

    #[cfg(feature = "rev2")]
    #[test]
    fn rev2_uses_direct_sleep_and_program_pins() {
        let mut board = Board::new();
        board.apply(
            0,
            Drive {
                mode: Mode::Programming,
                enabled: true,
            },
        );
        assert_eq!(board.io.pin_state(0), (true, true, false));
        board.inhibit(0);
        assert_eq!(board.io.pin_state(0), (false, false, false));
    }
}
