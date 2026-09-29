//! Rev1 USB CDC bench console. The port is local authority for DCC debugging.

use core::fmt::Write;
use core::sync::atomic::Ordering;
use dcc::locos::Apply;
use dst_01::{
    console::{Command, current_ma, parse},
    district::Mode,
};
use embassy_futures::{
    join::join,
    select::{Either, select},
};
use embassy_stm32::{
    Peri, bind_interrupts, peripherals,
    usb::{self, Driver},
};
use embassy_time::{Duration, Instant, Timer};
use embassy_usb::{
    Builder, Config,
    class::cdc_acm::{CdcAcmClass, State},
};
use heapless::String;

bind_interrupts!(struct Irqs { USB_UCPD1_2 => usb::InterruptHandler<peripherals::USB>; });

type UsbPins = (
    Peri<'static, peripherals::USB>,
    Peri<'static, peripherals::PA12>,
    Peri<'static, peripherals::PA11>,
);

// The final 16 bytes of rev1 RAM are reserved by both linker scripts. The ARC
// bootloader consumes this marker before starting the application.
const BOOT_KEY: u32 = 0x4152_4342;
const BOOT_REQUEST: *mut u32 = 0x2002_3ff8 as *mut u32;

fn request_bootloader() -> ! {
    unsafe {
        core::ptr::write_volatile(BOOT_REQUEST, BOOT_KEY);
        core::ptr::write_volatile(BOOT_REQUEST.add(1), !BOOT_KEY);
    }
    cortex_m::peripheral::SCB::sys_reset()
}

fn sampled_currents() -> ([u16; 4], [u16; 4]) {
    let counts = crate::hal::adc_counts();
    let milliamps = core::array::from_fn(|index| {
        let programming = dst_01::tasks::DISTRICTS[index]
            .status()
            .is_some_and(|status| status.config.mode == Mode::Programming);
        current_ma(counts[index], programming)
    });
    (counts, milliamps)
}

async fn send<'a>(
    class: &mut CdcAcmClass<'a, Driver<'a, peripherals::USB>>,
    message: &str,
) -> bool {
    for chunk in message.as_bytes().chunks(64) {
        if class.write_packet(chunk).await.is_err() {
            return false;
        }
    }
    true
}

async fn send_led_gpio<'a>(class: &mut CdcAcmClass<'a, Driver<'a, peripherals::USB>>) -> bool {
    // STM32G0B1 GPIO register map; read only for bench diagnostics.
    fn port(base: usize) -> (u32, u32, u32, u32) {
        unsafe {
            let reg = |offset| core::ptr::read_volatile((base + offset) as *const u32);
            (reg(0), reg(4), reg(0x14), reg(0x10))
        }
    }
    let c = port(0x5000_0800);
    let d = port(0x5000_0c00);
    let pins = [
        ("PC6 small green", c.0, c.1, c.2, c.3, 6),
        ("PC7 red", c.0, c.1, c.2, c.3, 7),
        ("PD9 green", d.0, d.1, d.2, d.3, 9),
        ("PD8 blue", d.0, d.1, d.2, d.3, 8),
    ];
    for (name, moder, otyper, odr, idr, bit) in pins {
        let mut msg: String<96> = String::new();
        let _ = write!(msg, "{} mode={} open_drain={} sink={} pin_high={}\r\n",
            name, (moder >> (bit * 2)) & 3, (otyper >> bit) & 1,
            (odr >> bit) & 1 == 0, (idr >> bit) & 1 != 0);
        if !send(class, &msg).await { return false; }
    }
    true
}

async fn console<'a>(class: &mut CdcAcmClass<'a, Driver<'a, peripherals::USB>>, own_id: u32) {
    let mut input = [0u8; 64];
    let mut line = [0u8; 96];
    let mut current_ms = 0u32;
    let mut next_current = 0u64;
    let mut sequence = 0u32;
    loop {
        class.wait_connection().await;
        let mut length = 0;
        let mut overflow = false;
        if !send(class, "ARC DST rev1; type help\r\n").await {
            continue;
        }
        let mut previous = [None; 4];
        loop {
            match select(
                class.read_packet(&mut input),
                Timer::after(Duration::from_millis(20)),
            )
            .await
            {
                Either::First(Err(_)) => break,
                Either::First(Ok(count)) => {
                    let mut disconnected = false;
                    for &byte in &input[..count] {
                        if byte == b'\r' || byte == b'\n' {
                            if overflow {
                                disconnected = !send(class, "ERR line too long\r\n").await;
                            } else if length > 0 {
                                let response = match core::str::from_utf8(&line[..length])
                                    .map_err(|_| "non-ASCII input")
                                    .and_then(parse)
                                {
                                    Ok(Command::Help) => {
                                        "help | led <auto|off|red|green|blue|white> | gpio | master | throttle <address> <f|r> <0..126> | current <ms> | status | bootloader | erase\r\n"
                                    }
                                    Ok(Command::Led(mode)) => {
                                        crate::LED_MODE.store(mode as u8, Ordering::Relaxed);
                                        "OK led mode\r\n"
                                    }
                                    Ok(Command::Gpio) => {
                                        disconnected |= !send_led_gpio(class).await;
                                        "OK gpio\r\n"
                                    }
                                    Ok(Command::Master) => {
                                        dst_01::sync::grant_local();
                                        "OK master granted (link power still required)\r\n"
                                    }
                                    Ok(Command::Throttle {
                                        address,
                                        forward,
                                        speed,
                                    }) => {
                                        sequence = sequence.wrapping_add(1);
                                        let result = dst_01::sync::apply(
                                            link::ThrottleSet {
                                                target: own_id,
                                                session: 0x5553_4201,
                                                sequence,
                                                address_kind: if address <= 127 { 0 } else { 1 },
                                                address,
                                                direction: u8::from(forward),
                                                speed,
                                                stop_mode: 0,
                                            },
                                            own_id,
                                        );
                                        match result {
                                            Apply::Changed | Apply::Repeated => "OK throttle\r\n",
                                            Apply::Full => "ERR locomotive table full\r\n",
                                            Apply::Rejected => "ERR throttle rejected\r\n",
                                        }
                                    }
                                    Ok(Command::Current(ms)) => {
                                        current_ms = ms;
                                        next_current = Instant::now().as_millis() + u64::from(ms);
                                        "OK current interval\r\n"
                                    }
                                    Ok(Command::Status) => {
                                        let (counts, milliamps) = sampled_currents();
                                        for index in 0..4 {
                                            if let Some(status) =
                                                dst_01::tasks::DISTRICTS[index].status()
                                            {
                                                let mut msg: String<128> = String::new();
                                                let _ = write!(
                                                    msg,
                                                    "district {} {:?} tripped={} current~{}mA adc={}\r\n",
                                                    index,
                                                    status.state,
                                                    status.tripped,
                                                    milliamps[index],
                                                    counts[index]
                                                );
                                                disconnected |= !send(class, &msg).await;
                                            }
                                        }
                                        "OK status\r\n"
                                    }
                                    Ok(Command::Bootloader) => {
                                        let _ =
                                            send(class, "OK rebooting to STM32 ROM USB DFU\r\n")
                                                .await;
                                        Timer::after(Duration::from_millis(100)).await;
                                        request_bootloader();
                                    }
                                    Ok(Command::Erase) => {
                                        let _ = send(class, "Erasing ARC bootloader page 0; USB will disconnect on success\r\n").await;
                                        Timer::after(Duration::from_millis(100)).await;
                                        crate::ERASE_REQUEST.send(()).await;
                                        match crate::ERASE_RESPONSE.receive().await {
                                            crate::EraseResult::Erased => "OK page 0 erased; rebooting to STM32 ROM USB DFU\r\n",
                                            crate::EraseResult::BootLocked => "ERR BOOT_LOCK forces flash boot; page 0 left intact\r\n",
                                            crate::EraseResult::Failed => "ERR flash page 0 erase failed\r\n",
                                        }
                                    }
                                    Err(error) => error,
                                };
                                disconnected |= !send(class, response).await;
                                if !response.ends_with("\r\n") {
                                    disconnected |= !send(class, "\r\n").await;
                                }
                            }
                            length = 0;
                            overflow = false;
                        } else if byte.is_ascii() && !overflow {
                            if length < line.len() {
                                line[length] = byte;
                                length += 1;
                            } else {
                                overflow = true;
                            }
                        } else {
                            overflow = true;
                        }
                    }
                    if disconnected {
                        break;
                    }
                }
                Either::Second(()) => {
                    for index in 0..4 {
                        let snapshot = dst_01::tasks::DISTRICTS[index]
                            .status()
                            .map(|status| (status.state, status.tripped));
                        if snapshot != previous[index] {
                            previous[index] = snapshot;
                            if let Some((state, tripped)) = snapshot {
                                let mut msg: String<96> = String::new();
                                let _ = write!(
                                    msg,
                                    "district {} -> {:?} tripped={}\r\n",
                                    index, state, tripped
                                );
                                if !send(class, &msg).await {
                                    break;
                                }
                            }
                        }
                    }
                    let now = Instant::now().as_millis();
                    if current_ms != 0 && now >= next_current {
                        let (_, milliamps) = sampled_currents();
                        let mut msg: String<96> = String::new();
                        let _ = write!(
                            msg,
                            "current ~mA: {} {} {} {}\r\n",
                            milliamps[0], milliamps[1], milliamps[2], milliamps[3]
                        );
                        if !send(class, &msg).await {
                            break;
                        }
                        next_current = now + u64::from(current_ms);
                    }
                }
            }
        }
    }
}

#[embassy_executor::task]
pub async fn run(pins: UsbPins, own_id: u32) {
    let (usb, dp, dm) = pins;
    let driver = Driver::new(usb, Irqs, dp, dm);
    let mut config = Config::new(0x1209, 0xA0C1);
    config.manufacturer = Some("ARC");
    config.product = Some("DST-01x4 rev1 console");
    let mut config_descriptor = [0; 256];
    let mut bos_descriptor = [0; 256];
    let mut msos_descriptor = [0; 256];
    let mut control_buf = [0; 64];
    let mut state = State::new();
    let mut builder = Builder::new(
        driver,
        config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut msos_descriptor,
        &mut control_buf,
    );
    let mut class = CdcAcmClass::new(&mut builder, &mut state, 64);
    let mut device = builder.build();
    join(device.run(), console(&mut class, own_id)).await;
}
