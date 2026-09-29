"""Track, district feeds, decoder commands, motion, and rail-boundary faults.

All positions and lengths are millimetres. Time advances only through ``step``;
the service supplies wall-clock ticks, while tests can use an exact clock.
"""

from dataclasses import dataclass
from functools import reduce


Feed = tuple[str, int]


@dataclass
class Piece:
    paths: dict[str, tuple[str, str, float]]
    feed: Feed
    selected: str | None = None

    def active_path(self, endpoint: str) -> tuple[str, tuple[str, str, float]] | None:
        for name, path in self.paths.items():
            if endpoint in path[:2] and (self.selected is None or name == self.selected):
                return name, path
        return None


@dataclass
class Loco:
    address_kind: int
    address: int
    piece: str
    path: str
    offset_mm: float
    facing: int
    pickup_mm: float
    idle_ma: int
    running_ma: int
    max_mm_s: float
    speed: int = 0
    direction: int = 1
    velocity_mm_s: float = 0
    blocked: str | None = None


class World:
    def __init__(self, fixture: dict):
        self.pieces: dict[str, Piece] = {}
        self.connections: dict[tuple[str, str], tuple[str, str, bool]] = {}
        self.locos: dict[str, Loco] = {}
        self.outputs: dict[Feed, dict] = {}
        self.link_power = False
        self.last_packet_s: float | None = None
        self.now_s = 0.0
        for name, entry in fixture["pieces"].items():
            feed = (entry["feed"]["board"], entry["feed"]["output"])
            paths = {route: (data["from"], data["to"], float(data["length_mm"]))
                     for route, data in entry["paths"].items()}
            if not paths or any(length <= 0 or a == b for a, b, length in paths.values()):
                raise ValueError(f"{name}: invalid path")
            selected = entry.get("selected")
            if selected is not None and selected not in paths:
                raise ValueError(f"{name}: unknown selected path")
            if selected is None and len(paths) != 1:
                raise ValueError(f"{name}: multiple paths need a turnout selection")
            self.pieces[name] = Piece(paths, feed, selected)
        for edge in fixture["connections"]:
            a = (edge["a"]["piece"], edge["a"]["port"])
            b = (edge["b"]["piece"], edge["b"]["port"])
            if a == b or any(key in self.connections for key in (a, b)):
                raise ValueError("duplicate or self-connected endpoint")
            for piece, port in (a, b):
                if piece not in self.pieces or not any(port in path[:2] for path in self.pieces[piece].paths.values()):
                    raise ValueError(f"unknown endpoint {piece}:{port}")
            swapped = edge["rails"] == "swapped"
            if edge["rails"] not in ("straight", "swapped"):
                raise ValueError("rails must be straight or swapped")
            self.connections[a] = (*b, swapped)
            self.connections[b] = (*a, swapped)
        for name, entry in fixture.get("locos", {}).items():
            loco = Loco(**entry)
            if loco.piece not in self.pieces or loco.path not in self.pieces[loco.piece].paths:
                raise ValueError(f"{name}: unknown location")
            if not 0 <= loco.offset_mm <= self.pieces[loco.piece].paths[loco.path][2]:
                raise ValueError(f"{name}: offset outside path")
            if loco.facing not in (-1, 1) or loco.pickup_mm < 0:
                raise ValueError(f"{name}: invalid orientation or pickup span")
            if loco.max_mm_s <= 0 or min(loco.idle_ma, loco.running_ma) < 0:
                raise ValueError(f"{name}: invalid speed or current model")
            self.locos[name] = loco

    def set_output(self, feed: Feed, enabled: bool, mode: str, phase: int = 0) -> None:
        if phase not in (0, 1) or mode not in ("disabled", "synced", "programming"):
            raise ValueError("invalid output state")
        self.outputs[feed] = {"enabled": enabled, "mode": mode, "phase": phase}

    def powered(self, feed: Feed) -> bool:
        output = self.outputs.get(feed, {})
        return bool(self.link_power and output.get("enabled") and output.get("mode") == "synced"
                    and self.last_packet_s is not None)

    def receive_packet(self, bits: str) -> None:
        if not self.link_power:
            return
        data = decode_packet(bits)
        if data is None:
            return
        self.last_packet_s = self.now_s
        command = decode_speed(data)
        if command is None:
            return
        address_kind, address, direction, speed = command
        for loco in self.locos.values():
            if (loco.address_kind, loco.address) == (address_kind, address) and self.powered(self.pieces[loco.piece].feed):
                loco.direction, loco.speed = direction, speed

    def set_turnout(self, piece: str, path: str) -> None:
        target = self.pieces[piece]
        if target.selected is None or path not in target.paths:
            raise ValueError("not a turnout path")
        if any(loco.piece == piece for loco in self.locos.values()):
            raise ValueError("turnout occupied")
        target.selected = path

    def bridge(self, loco: Loco) -> tuple[Feed, bool] | None:
        piece = self.pieces[loco.piece]
        start, end, length = piece.paths[loco.path]
        for endpoint, distance in ((start, loco.offset_mm), (end, length - loco.offset_mm)):
            if distance > loco.pickup_mm / 2:
                continue
            neighbor = self.connections.get((loco.piece, endpoint))
            if neighbor is None:
                continue
            other_name, other_port, swapped = neighbor
            other = self.pieces[other_name]
            if other.feed == piece.feed or other.active_path(other_port) is None:
                continue
            return other.feed, swapped
        return None

    def load(self, feed: Feed) -> tuple[int, bool]:
        if not self.powered(feed):
            return 0, False
        current = 0
        fault = False
        for loco in self.locos.values():
            own = self.pieces[loco.piece].feed
            bridge = self.bridge(loco)
            attached = own == feed or bridge is not None and bridge[0] == feed
            if not attached:
                continue
            if bridge is not None and self.powered(own) and self.powered(bridge[0]):
                a = self.outputs[own]["phase"]
                b = self.outputs[bridge[0]]["phase"]
                if (a ^ b) != int(bridge[1]):
                    fault = True
            current += loco.idle_ma + round(loco.running_ma * abs(loco.velocity_mm_s) / loco.max_mm_s)
        return (5_000 if fault else current), fault

    def step(self, seconds: float) -> None:
        if seconds < 0:
            raise ValueError("time cannot go backwards")
        self.now_s += seconds
        for loco in self.locos.values():
            feed = self.pieces[loco.piece].feed
            target = loco.max_mm_s * loco.speed / 126 if self.powered(feed) else 0
            target *= loco.facing if loco.direction else -loco.facing
            loco.velocity_mm_s = target  # Initial model: no inertia.
            travel = target * seconds
            loco.blocked = None
            for _ in range(32):
                path = self.pieces[loco.piece].paths[loco.path]
                next_offset = loco.offset_mm + travel
                if 0 <= next_offset <= path[2]:
                    loco.offset_mm = next_offset
                    break
                exit_port = path[1] if travel > 0 else path[0]
                remaining = next_offset - path[2] if travel > 0 else next_offset
                neighbor = self.connections.get((loco.piece, exit_port))
                if neighbor is None:
                    loco.offset_mm = path[2] if travel > 0 else 0
                    loco.blocked = "track_end"
                    loco.velocity_mm_s = 0
                    break
                next_piece, entry, _ = neighbor
                route = self.pieces[next_piece].active_path(entry)
                if route is None:
                    loco.offset_mm = path[2] if travel > 0 else 0
                    loco.blocked = "turnout"
                    loco.velocity_mm_s = 0
                    break
                name, next_path = route
                loco.piece, loco.path = next_piece, name
                if entry == next_path[0]:
                    loco.offset_mm = 0
                    travel = abs(remaining)
                else:
                    loco.offset_mm = next_path[2]
                    travel = -abs(remaining)
                loco.facing = (1 if travel > 0 else -1) * (1 if loco.direction else -1)
                # A dead output cannot carry the loco further.
                if not self.powered(self.pieces[next_piece].feed):
                    loco.velocity_mm_s = 0
                    break
            else:
                raise ValueError("too many track crossings in one tick")

    def snapshot(self) -> dict:
        return {"locos": {name: {"piece": loco.piece, "path": loco.path,
                                 "offset_mm": round(loco.offset_mm, 3),
                                 "speed": loco.speed, "velocity_mm_s": loco.velocity_mm_s,
                                 "blocked": loco.blocked}
                          for name, loco in self.locos.items()},
                "outputs": {f"{board}:{output}": {**state, "current_ma": self.load((board, output))[0],
                                                    "fault": self.load((board, output))[1]}
                            for (board, output), state in self.outputs.items()}}


def decode_packet(bits: str) -> bytes | None:
    if len(bits) not in (42, 51, 60) or not bits.startswith("1" * 14) or bits[-1:] != "1":
        return None
    data = []
    for index in range(14, len(bits) - 1, 9):
        if bits[index] != "0" or any(bit not in "01" for bit in bits[index + 1:index + 9]):
            return None
        data.append(int(bits[index + 1:index + 9], 2))
    if not data or reduce(lambda a, b: a ^ b, data, 0):
        return None
    return bytes(data)


def decode_speed(data: bytes) -> tuple[int, int, int, int] | None:
    if len(data) == 4 and 1 <= data[0] <= 127:
        kind, address, instruction = 0, data[0], data[1:3]
    elif len(data) == 5 and 0xC0 <= data[0] <= 0xE7:
        kind, address, instruction = 1, ((data[0] & 0x3F) << 8) | data[1], data[2:4]
    else:
        return None
    if instruction[0] != 0x3F:
        return None
    code = instruction[1] & 0x7F
    return kind, address, instruction[1] >> 7, 0 if code <= 1 else code - 1
