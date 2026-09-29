#!/usr/bin/env python3
"""Package rev1 ELF images for one ROM DFU write at 0x08000000."""

import argparse
import pathlib
import struct

FLASH = 0x08000000
ACTIVE = 0x08004000
DFU = 0x08040000
STATE = 0x0807E000
RAM_TOP = 0x20023FF0
PARTITIONS = {
    "__bootloader_active_start": 0x4000,
    "__bootloader_active_end": 0x40000,
    "__bootloader_dfu_start": 0x40000,
    "__bootloader_dfu_end": 0x7E000,
    "__bootloader_state_start": 0x7E000,
    "__bootloader_state_end": 0x80000,
}


def check_partitions(path: pathlib.Path, required: tuple[str, ...]) -> None:
    elf = path.read_bytes()
    shoff = struct.unpack_from("<I", elf, 32)[0]
    shsize, shcount = struct.unpack_from("<HH", elf, 46)
    if shsize < 40 or shoff + shsize * shcount > len(elf):
        raise ValueError(f"{path}: invalid section headers")
    sections = [struct.unpack_from("<IIIIIIIIII", elf, shoff + i * shsize) for i in range(shcount)]
    found = {}
    for section in sections:
        if section[1] != 2:  # SHT_SYMTAB
            continue
        strings = sections[section[6]]
        table = elf[strings[4] : strings[4] + strings[5]]
        offset, size, entry_size = section[4], section[5], section[9]
        if entry_size < 16 or offset + size > len(elf):
            raise ValueError(f"{path}: invalid symbol table")
        for pos in range(offset, offset + size, entry_size):
            name_offset, value = struct.unpack_from("<II", elf, pos)
            if name_offset >= len(table):
                continue
            name = table[name_offset :].split(b"\0", 1)[0].decode("ascii", errors="replace")
            if name in PARTITIONS:
                found[name] = value
    for name in required:
        if found.get(name) != PARTITIONS[name]:
            raise ValueError(f"{path}: {name} is {found.get(name)}, expected {PARTITIONS[name]:#x}")


def flash_bytes(path: pathlib.Path, start: int, end: int) -> bytes:
    elf = path.read_bytes()
    if len(elf) < 52 or elf[:6] != b"\x7fELF\x01\x01" or struct.unpack_from("<H", elf, 18)[0] != 40:
        raise ValueError(f"{path}: expected little-endian ARM ELF32")
    phoff = struct.unpack_from("<I", elf, 28)[0]
    phsize, phcount = struct.unpack_from("<HH", elf, 42)
    if phsize < 32 or phoff + phsize * phcount > len(elf):
        raise ValueError(f"{path}: invalid program headers")
    segments = []
    for index in range(phcount):
        kind, offset, _, physical, size, _, _, _ = struct.unpack_from(
            "<IIIIIIII", elf, phoff + index * phsize
        )
        if kind != 1 or size == 0:
            continue
        if offset + size > len(elf):
            raise ValueError(f"{path}: truncated load segment")
        data = elf[offset : offset + size]
        # LLD includes ELF headers as a LOAD segment below the application
        # vector table. These are file metadata, not target flash contents.
        if start == ACTIVE and physical == FLASH and offset == 0 and data.startswith(b"\x7fELF"):
            continue
        if physical < start or physical + size > end:
            raise ValueError(f"{path}: load segment outside its flash partition at {physical:#010x}")
        segments.append((physical, data))
    if not segments or min(address for address, _ in segments) != start:
        raise ValueError(f"{path}: missing vector table at {start:#010x}")
    result = bytearray(b"\xff" * (max(address + len(data) for address, data in segments) - start))
    used = bytearray(len(result))
    for address, data in segments:
        offset = address - start
        if any(used[offset : offset + len(data)]):
            raise ValueError(f"{path}: overlapping load segments")
        result[offset : offset + len(data)] = data
        used[offset : offset + len(data)] = b"\x01" * len(data)
    stack, reset = struct.unpack_from("<II", result)
    if not 0x20000000 < stack <= RAM_TOP or not (reset & 1 and start <= reset & ~1 < start + len(result)):
        raise ValueError(f"{path}: invalid stack or reset vector")
    return bytes(result)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("boot_elf", type=pathlib.Path)
    parser.add_argument("app_elf", type=pathlib.Path)
    parser.add_argument("output_dir", type=pathlib.Path)
    args = parser.parse_args()
    check_partitions(args.boot_elf, tuple(PARTITIONS))
    check_partitions(
        args.app_elf,
        ("__bootloader_dfu_start", "__bootloader_dfu_end", "__bootloader_state_start", "__bootloader_state_end"),
    )
    boot = flash_bytes(args.boot_elf, FLASH, ACTIVE)
    app = flash_bytes(args.app_elf, ACTIVE, DFU)
    combined = boot + b"\xff" * (ACTIVE - FLASH - len(boot)) + app
    args.output_dir.mkdir(parents=True, exist_ok=True)
    (args.output_dir / "arc-boot-rev1.bin").write_bytes(boot)
    (args.output_dir / "dst-rev1.bin").write_bytes(app)
    (args.output_dir / "dst-rev1-initial.bin").write_bytes(combined)
    print(f"boot: {len(boot)} bytes at {FLASH:#010x}")
    print(f"application: {len(app)} bytes at {ACTIVE:#010x}")
    print(f"combined: {len(combined)} bytes at {FLASH:#010x}; DFU/state remain untouched")


if __name__ == "__main__":
    main()
