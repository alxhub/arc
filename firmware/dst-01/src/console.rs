//! Bounded, ASCII line command grammar for the rev1 USB debug port.

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Led(LedMode),
    Gpio,
    Master,
    Throttle {
        address: u16,
        forward: bool,
        speed: u8,
    },
    Bootloader,
    Erase,
    Current(u32),
    Status,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LedMode {
    Auto = 0,
    Off = 1,
    Red = 2,
    Green = 3,
    Blue = 4,
    White = 5,
}

impl LedMode {
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Off,
            2 => Self::Red,
            3 => Self::Green,
            4 => Self::Blue,
            5 => Self::White,
            _ => Self::Auto,
        }
    }

    pub fn color(self) -> Option<(bool, bool, bool)> {
        match self {
            Self::Auto => None,
            Self::Off => Some((false, false, false)),
            Self::Red => Some((true, false, false)),
            Self::Green => Some((false, true, false)),
            Self::Blue => Some((false, false, true)),
            Self::White => Some((true, true, true)),
        }
    }
}

/// Rev1 nominal DRV8874 IPROPI conversion. The ADC sees the terminated node
/// directly, referenced to nominal 3.3 V. TI specifies 450 µA/A typical.
pub fn current_ma(adc_counts: u16, programming: bool) -> u16 {
    let resistance_ohms = if programming { 22_100u64 } else { 2_210u64 };
    let numerator = u64::from(adc_counts) * 3_300 * 1_000_000;
    let denominator = 4_095 * resistance_ohms * 450;
    ((numerator + denominator / 2) / denominator) as u16
}

pub fn parse(line: &str) -> Result<Command, &'static str> {
    let mut words = line.split_ascii_whitespace();
    let command = words.next().ok_or("empty command")?;
    let parsed = match command {
        "help" => Command::Help,
        "gpio" => Command::Gpio,
        "led" => {
            let mode = match words.next() {
                Some("auto") => LedMode::Auto,
                Some("off") => LedMode::Off,
                Some("red") => LedMode::Red,
                Some("green") => LedMode::Green,
                Some("blue") => LedMode::Blue,
                Some("white") => LedMode::White,
                _ => return Err("usage: led <auto|off|red|green|blue|white>"),
            };
            Command::Led(mode)
        }
        "master" => Command::Master,
        "bootloader" => Command::Bootloader,
        "erase" => Command::Erase,
        "status" => Command::Status,
        "current" => {
            let ms = words
                .next()
                .ok_or("usage: current <ms>")?
                .parse::<u32>()
                .map_err(|_| "invalid interval")?;
            Command::Current(ms)
        }
        "throttle" => {
            let address = words
                .next()
                .ok_or("usage: throttle <address> <f|r> <0..126>")?
                .parse::<u16>()
                .map_err(|_| "invalid address")?;
            let forward = match words.next() {
                Some("f") => true,
                Some("r") => false,
                _ => return Err("direction must be f or r"),
            };
            let speed = words
                .next()
                .ok_or("missing speed")?
                .parse::<u8>()
                .map_err(|_| "invalid speed")?;
            if !(1..=10239).contains(&address) || speed > 126 {
                return Err("address or speed out of range");
            }
            Command::Throttle {
                address,
                forward,
                speed,
            }
        }
        _ => return Err("unknown command; type help"),
    };
    if words.next().is_some() {
        return Err("too many arguments");
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commands_are_strict_and_bounded() {
        assert_eq!(parse("master\r"), Ok(Command::Master));
        assert_eq!(
            parse("throttle 3 r 126"),
            Ok(Command::Throttle {
                address: 3,
                forward: false,
                speed: 126
            })
        );
        assert!(parse("throttle 0 f 1").is_err());
        assert!(parse("throttle 3 f 127").is_err());
        assert!(parse("bootloader now").is_err());
        assert_eq!(parse("erase"), Ok(Command::Erase));
        assert!(parse("erase now").is_err());
        assert_eq!(parse("current 0"), Ok(Command::Current(0)));
        assert_eq!(parse("led green"), Ok(Command::Led(LedMode::Green)));
        assert_eq!(parse("gpio"), Ok(Command::Gpio));
        assert!(parse("led purple").is_err());
    }

    #[test]
    fn rev1_nominal_current_scales_follow_the_two_sense_resistors() {
        assert_eq!(current_ma(0, false), 0);
        assert!((995..=1005).contains(&current_ma(1234, false)));
        assert!((95..=105).contains(&current_ma(1234, true)));
        assert!((2980..=3000).contains(&current_ma(3688, false)));
    }
}
