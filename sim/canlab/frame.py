"""CAN FD frame values for the simulator's private TCP transport."""

from dataclasses import dataclass
import json

MAX_CAN_ID = 0x1FFFFFFF
CAN_FD_LENGTHS = set(range(9)) | {12, 16, 20, 24, 32, 48, 64}


@dataclass(frozen=True)
class Frame:
    can_id: int
    data: bytes

    def __post_init__(self) -> None:
        if not 0 <= self.can_id <= MAX_CAN_ID:
            raise ValueError("CAN ID must be a 29-bit extended identifier")
        if len(self.data) not in CAN_FD_LENGTHS:
            raise ValueError("data length is not representable by a CAN FD DLC")

    def to_line(self) -> bytes:
        return (json.dumps({"id": self.can_id, "data": self.data.hex()}, separators=(",", ":")) + "\n").encode("ascii")

    @classmethod
    def from_line(cls, line: bytes) -> "Frame":
        value = json.loads(line)
        if not isinstance(value, dict) or type(value.get("id")) is not int or not isinstance(value.get("data"), str):
            raise ValueError("frame requires integer id and hexadecimal data")
        return cls(value["id"], bytes.fromhex(value["data"]))
