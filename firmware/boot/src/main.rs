//! ARC power-fail-safe image swap, shared by DST and PSU MCUs.
#![no_std]
#![no_main]

#[cfg(not(any(feature = "g0b1", feature = "c092")))]
compile_error!("select g0b1 or c092");
#[cfg(all(feature = "g0b1", feature = "c092"))]
compile_error!("select only one chip");

use core::cell::RefCell;
use embassy_boot_stm32::{BootLoader, BootLoaderConfig};
use embassy_stm32::flash::Flash;
use embassy_sync::blocking_mutex::{Mutex, raw::NoopRawMutex};
use panic_halt as _;

#[cfg(feature = "g0b1")]
const BOOT_KEY: u32 = 0x4152_4342;
#[cfg(feature = "g0b1")]
const REQUEST: *mut u32 = 0x2002_3ff8 as *mut u32;

#[cfg(feature = "g0b1")]
fn request_rom_bootloader() -> ! {
    unsafe {
        core::ptr::write_volatile(REQUEST, BOOT_KEY);
        core::ptr::write_volatile(REQUEST.add(1), !BOOT_KEY);
    }
    cortex_m::peripheral::SCB::sys_reset()
}

#[cfg(feature = "g0b1")]
fn enter_requested_rom_bootloader() {
    let requested = unsafe {
        core::ptr::read_volatile(REQUEST) == BOOT_KEY
            && core::ptr::read_volatile(REQUEST.add(1)) == !BOOT_KEY
    };
    if requested {
        unsafe {
            core::ptr::write_volatile(REQUEST, 0);
            core::ptr::write_volatile(REQUEST.add(1), 0);
            // Enable SYSCFG, then map STM32G0B1 system memory at address zero.
            let enable = 0x4002_1040 as *mut u32;
            core::ptr::write_volatile(enable, core::ptr::read_volatile(enable) | 1);
            let remap = 0x4001_0000 as *mut u32;
            core::ptr::write_volatile(remap, (core::ptr::read_volatile(remap) & !3) | 1);
            cortex_m::asm::bootload(0x1fff_0000 as *const u32);
        }
    }
}

#[cfg(feature = "g0b1")]
fn application_vectors_valid() -> bool {
    let stack = unsafe { core::ptr::read_volatile(0x0800_4000 as *const u32) };
    let reset = unsafe { core::ptr::read_volatile(0x0800_4004 as *const u32) };
    (0x2000_0000..=0x2002_3ff0).contains(&stack)
        && reset & 1 != 0
        && (0x0800_4000..0x0804_0000).contains(&(reset & !1))
}

#[cortex_m_rt::entry]
fn main() -> ! {
    #[cfg(feature = "g0b1")]
    enter_requested_rom_bootloader();
    let p = embassy_stm32::init(Default::default());
    let flash = Mutex::<NoopRawMutex, _>::new(RefCell::new(Flash::new_blocking(p.FLASH)));
    let config = BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let boot = match BootLoader::try_prepare::<_, _, _, 2048>(config) {
        Ok(boot) => boot,
        Err(_) => {
            #[cfg(feature = "g0b1")]
            request_rom_bootloader();
            #[cfg(feature = "c092")]
            panic!("Boot prepare error");
        }
    };
    #[cfg(feature = "g0b1")]
    if !application_vectors_valid() {
        request_rom_bootloader();
    }
    // The active partition starts after the protected 16 KiB bootloader.
    unsafe { boot.load(0x0800_4000) }
}
