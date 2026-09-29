// Shared linker layout for ARC boards using embassy-boot.
use std::{env, fs, path::PathBuf};

pub fn write(g0b1: bool, bootloader: bool) {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let (ram_size, active_len, dfu_len, state_len) = if g0b1 {
        ("0x23ff0", "240K", "248K", "8K")
    } else {
        ("30K", "54K", "56K", "2K")
    };
    let active = 0x0800_4000u32;
    let dfu = active + if g0b1 { 240 * 1024 } else { 54 * 1024 };
    let state = dfu + if g0b1 { 248 * 1024 } else { 56 * 1024 };
    // Embassy's BlockingPartition takes offsets from the NorFlash start.
    // Boot and applications pass the whole internal Flash to the updater.
    let active_offset = active - 0x0800_0000;
    let dfu_offset = dfu - 0x0800_0000;
    let state_offset = state - 0x0800_0000;
    let (flash_origin, flash_len) = if bootloader {
        (0x0800_0000u32, "16K")
    } else {
        (active, active_len)
    };
    let script = format!(
        "\
MEMORY {{
  FLASH : ORIGIN = 0x{flash_origin:08x}, LENGTH = {flash_len}
  RAM : ORIGIN = 0x20000000, LENGTH = {ram_size}
  ACTIVE : ORIGIN = 0x{active:08x}, LENGTH = {active_len}
  DFU : ORIGIN = 0x{dfu:08x}, LENGTH = {dfu_len}
  BOOTLOADER_STATE : ORIGIN = 0x{state:08x}, LENGTH = {state_len}
}}
__bootloader_active_start = 0x{active_offset:x};
__bootloader_active_end = 0x{active_offset:x} + LENGTH(ACTIVE);
__bootloader_dfu_start = 0x{dfu_offset:x};
__bootloader_dfu_end = 0x{dfu_offset:x} + LENGTH(DFU);
__bootloader_state_start = 0x{state_offset:x};
__bootloader_state_end = 0x{state_offset:x} + LENGTH(BOOTLOADER_STATE);
"
    );
    fs::write(out.join("memory.x"), script).expect("write memory.x");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=../shared/update_layout.rs");
}
