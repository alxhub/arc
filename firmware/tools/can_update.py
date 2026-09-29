#!/usr/bin/env python3
"""Send an ARC firmware image through the stop-and-wait CAN FD update protocol."""

import argparse
import pathlib
import secrets
import socket
import struct
import sys
import zlib

CAN_EFF_FLAG = 0x80000000
CAN_RAW_FD_FRAMES = 5
SOL_CAN_RAW = 101
FRAME = struct.Struct("=IBBBB64s")
KIND = {"dst-rev1": 1, "dst-rev2": 2, "psu": 3}
CAPACITY = {"dst-rev1": 240 * 1024, "dst-rev2": 54 * 1024, "psu": 54 * 1024}
RAM_END = {"dst-rev1": 0x20023FF0, "dst-rev2": 0x20007800, "psu": 0x20007800}


def encode_frame(can_id: int, data: bytes) -> bytes:
    if len(data) not in (12, 20, 64):
        raise ValueError("invalid CAN FD length")
    return FRAME.pack(can_id | CAN_EFF_FLAG, len(data), 0, 0, 0, data.ljust(64, b"\x00"))


def validate_image(image: bytes, kind: str) -> None:
    if len(image) < 8 or len(image) > CAPACITY[kind]:
        raise ValueError(f"image must be 8..{CAPACITY[kind]} bytes")
    stack, reset = struct.unpack_from("<II", image)
    if not 0x20000000 <= stack <= RAM_END[kind]:
        raise ValueError("image stack pointer does not match target RAM")
    if not (reset & 1 and 0x08004000 <= reset & ~1 < 0x08004000 + len(image)):
        raise ValueError("image reset vector is outside the active partition")


def begin(target: int, kind: str, session: int, image: bytes) -> bytes:
    return (b"\x01\x01" + target.to_bytes(3, "big") + bytes([KIND[kind]])
            + struct.pack(">III", session, len(image), zlib.crc32(image)) + b"\x00\x00")


def chunk(target: int, session: int, offset: int, data: bytes) -> bytes:
    if not 1 <= len(data) <= 48 or offset % 48:
        raise ValueError("invalid chunk")
    return (b"\x01\x02" + target.to_bytes(3, "big") + struct.pack(">II", session, offset)
            + bytes([len(data)]) + data.ljust(48, b"\xff") + b"\xff\xff")


def finish(target: int, session: int) -> bytes:
    return b"\x01\x03" + target.to_bytes(3, "big") + struct.pack(">I", session) + b"\x00" * 3


def wait_status(bus: socket.socket, target: int, session: int) -> tuple[int, int, int]:
    while True:
        frame = bus.recv(FRAME.size)
        if len(frame) != FRAME.size:
            continue
        can_id, length, _, _, _, data = FRAME.unpack(frame)
        if can_id != CAN_EFF_FLAG | (0x17 << 24) | target or length != 12:
            continue
        if data[:2] != b"\x01\x01" or struct.unpack_from(">I", data, 2)[0] != session:
            continue
        next_offset = struct.unpack_from(">I", data, 6)[0]
        return data[10], next_offset, data[11]


def exchange(bus: socket.socket, sender: int, target: int, session: int,
             data: bytes, wanted: int, expected_offset: int) -> None:
    for attempt in range(4):
        bus.send(encode_frame((0x16 << 24) | sender, data))
        try:
            phase, offset, error = wait_status(bus, target, session)
        except TimeoutError:
            if attempt == 3:
                raise RuntimeError("target did not acknowledge update frame") from None
            continue
        if phase == 4:
            raise RuntimeError(f"target rejected update: error {error}, next offset {offset}")
        if phase != wanted or offset != expected_offset or error:
            raise RuntimeError(f"unexpected update status: phase={phase}, offset={offset}, error={error}")
        return


def parse_hex_id(value: str) -> int:
    number = int(value, 16)
    if not 0 <= number <= 0xFFFFFF:
        raise argparse.ArgumentTypeError("network ID must fit 24 bits")
    return number


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--interface", default="can0", help="Linux SocketCAN FD interface")
    parser.add_argument("--sender-id", type=parse_hex_id, required=True, help="host network ID as six hex digits")
    parser.add_argument("--target-id", type=parse_hex_id, required=True, help="board network ID as six hex digits")
    parser.add_argument("--kind", choices=KIND, required=True)
    parser.add_argument("image", type=pathlib.Path, help="raw application .bin linked at 0x08004000")
    args = parser.parse_args()
    if args.sender_id == args.target_id:
        parser.error("sender and target network IDs must differ")
    image = args.image.read_bytes()
    validate_image(image, args.kind)
    session = secrets.randbelow(0xFFFFFFFF) + 1
    with socket.socket(socket.PF_CAN, socket.SOCK_RAW, socket.CAN_RAW) as bus:
        bus.setsockopt(SOL_CAN_RAW, CAN_RAW_FD_FRAMES, 1)
        # Begin erases the entire staging partition before its acknowledgement.
        bus.settimeout(30)
        bus.bind((args.interface,))
        exchange(bus, args.sender_id, args.target_id, session,
                 begin(args.target_id, args.kind, session, image), 1, 0)
        bus.settimeout(5)
        for offset in range(0, len(image), 48):
            data = image[offset:offset + 48]
            exchange(bus, args.sender_id, args.target_id, session,
                     chunk(args.target_id, session, offset, data), 2, offset + len(data))
            print(f"\r{offset + len(data)}/{len(image)} bytes", end="", flush=True)
        exchange(bus, args.sender_id, args.target_id, session,
                 finish(args.target_id, session), 3, len(image))
    print("\nTarget accepted image and is rebooting")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ValueError) as error:
        print(f"can-update: {error}", file=sys.stderr)
        raise SystemExit(1) from None
