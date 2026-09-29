"""Live physical fixture for host firmware; no layoutd or MQTT dependency."""

import argparse
import asyncio
import json
import logging
from pathlib import Path

from sim.world.model import World


LOG = logging.getLogger("sim.world")


class WorldService:
    def __init__(self, world: World):
        self.world = world
        self.tick = 0
        self.sampled: dict[tuple[str, int], int] = {}
        self.samples = asyncio.Condition()
        self.advance_lock = asyncio.Lock()
        self.clock_acks: dict[str, int] = {}
        self.clock_owners: dict[str, asyncio.StreamWriter] = {}
        self.signal_tick = 0
        self.source_barriers: dict[str, int] = {}

    async def advance(self, milliseconds: int) -> dict:
        if type(milliseconds) is not int or not 0 <= milliseconds <= 60_000:
            raise ValueError("advance requires milliseconds 0..60000")
        async with self.advance_lock:
            async with self.samples:
                for _ in range(milliseconds):
                    self.tick += 1
                    self.world.step(0.001)
                    self.samples.notify_all()
                    expected = set(self.sampled)
                    for name in ("can", "layoutd", "psu", "node_a", "node_b", "node_collision"):
                        if name not in self.clock_acks or (name == "can" and self.tick % 5):
                            continue
                        try:
                            await asyncio.wait_for(
                                self.samples.wait_for(
                                    lambda: self.clock_acks.get(name, -1) >= self.tick
                                ), timeout=2)
                        except asyncio.TimeoutError as error:
                            raise ValueError(f"{name} did not acknowledge world tick {self.tick}") from error
                        if name == "psu":
                            try:
                                await asyncio.wait_for(
                                    self.samples.wait_for(lambda: self.signal_tick >= self.tick),
                                    timeout=2)
                            except asyncio.TimeoutError as error:
                                raise ValueError(f"signals did not reach world at tick {self.tick}") from error
                        if name == "node_a":
                            try:
                                await asyncio.wait_for(
                                    self.samples.wait_for(lambda: self.source_barriers.get("node_a", -1) >= self.tick),
                                    timeout=2)
                            except asyncio.TimeoutError as error:
                                raise ValueError(f"node_a signals did not reach world at tick {self.tick}") from error
                    if expected:
                        try:
                            await asyncio.wait_for(
                                self.samples.wait_for(
                                    lambda: all(self.sampled.get(feed, -1) >= self.tick for feed in expected)
                                ), timeout=2)
                        except asyncio.TimeoutError as error:
                            raise ValueError(f"outputs did not sample world tick {self.tick}") from error
        return {"ok": True, "time_ms": self.tick}

    async def clock_wait(self, name: str, after_ms: int) -> dict:
        period = 5 if name == "can" else 1
        if name not in self.clock_acks or type(after_ms) is not int or after_ms < 0 or after_ms % period:
            raise ValueError("invalid clock wait")
        def ready() -> bool:
            target = after_ms + period
            if self.tick < target:
                return False
            if name != "can" and target % 5 == 0 and "can" in self.clock_acks:
                if self.clock_acks["can"] < target:
                    return False
            if name in ("node_a", "node_b", "node_collision") and "psu" in self.clock_acks:
                if self.clock_acks["psu"] < target or self.signal_tick < target:
                    return False
            if name in ("node_b", "node_collision") and "node_a" in self.clock_acks:
                return self.clock_acks["node_a"] >= target and self.source_barriers.get("node_a", -1) >= target
            return True
        async with self.samples:
            try:
                await asyncio.wait_for(
                    self.samples.wait_for(ready), timeout=0.05
                )
            except asyncio.TimeoutError:
                return {"ok": True, "time_ms": after_ms, "pending": True}
            return {"ok": True, "time_ms": self.tick}

    async def signals(self, address: str) -> None:
        while True:
            try:
                reader, writer = await asyncio.open_connection(*split_address(address))
                try:
                    while line := await reader.readline():
                        signal = json.loads(line)
                        if signal.get("kind") == "power":
                            self.world.link_power = signal["enabled"]
                            if not self.world.link_power:
                                self.world.last_packet_s = None
                        elif signal.get("kind") == "dcc_packet":
                            self.world.receive_packet(signal["bits"])
                        elif signal.get("kind") == "barrier":
                            async with self.samples:
                                self.signal_tick = max(self.signal_tick, signal["time_ms"])
                                if isinstance(signal.get("source"), str):
                                    source = signal["source"]
                                    self.source_barriers[source] = max(self.source_barriers.get(source, -1), signal["time_ms"])
                                self.samples.notify_all()
                finally:
                    writer.close()
                    await writer.wait_closed()
            except (OSError, ValueError, json.JSONDecodeError) as error:
                LOG.warning("signal connection: %s", error)
            self.world.link_power = False
            self.world.last_packet_s = None
            await asyncio.sleep(0.1)

    def request(self, command: dict) -> dict:
        op = command.get("op")
        if op == "status":
            return {"ok": True, "time_ms": self.tick,
                    "clock_participants": sorted(self.clock_acks), **self.world.snapshot()}
        if op == "clock_register":
            name = command.get("name")
            if name not in ("can", "layoutd", "psu", "node_a", "node_b", "node_collision"):
                raise ValueError("invalid clock participant")
            period = 5 if name == "can" else 1
            last_boundary = self.tick - self.tick % period
            self.clock_acks[name] = last_boundary
            return {"ok": True, "time_ms": last_boundary}
        if op == "clock_ack":
            tick = command.get("time_ms")
            name = command.get("name")
            if name not in self.clock_acks or type(tick) is not int or tick != self.tick or (name == "can" and tick % 5):
                raise ValueError("invalid clock acknowledgement")
            self.clock_acks[name] = tick
            return {"ok": True}
        if op == "sample":
            feed = (command["board"], command["output"])
            self.world.set_output(feed, command["enabled"], command["mode"], command.get("phase", 0))
            current, fault = self.world.load(feed)
            return {"ok": True, "current_ma": current, "fault": fault}
        if op == "sample_all":
            board = command["board"]
            outputs = command["outputs"]
            if not isinstance(outputs, list) or len(outputs) != 4:
                raise ValueError("sample_all requires four outputs")
            for index, state in enumerate(outputs):
                self.world.set_output((board, index), state["enabled"], state["mode"], state.get("phase", 0))
            samples = []
            for index in range(4):
                current, fault = self.world.load((board, index))
                samples.append({"current_ma": current, "fault": fault})
            return {"ok": True, "samples": samples}
        if op == "set_turnout":
            self.world.set_turnout(command["piece"], command["path"])
            return {"ok": True}
        if op == "place":
            loco = self.world.locos[command["loco"]]
            piece, path, offset = command["piece"], command["path"], command["offset_mm"]
            length = self.world.pieces[piece].paths[path][2]
            if not 0 <= offset <= length:
                raise ValueError("offset outside path")
            loco.piece, loco.path, loco.offset_mm = piece, path, offset
            loco.velocity_mm_s = 0
            loco.blocked = None
            return {"ok": True}
        raise ValueError("unknown operation")

    async def client(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        registered = None
        try:
            while line := await reader.readline():
                try:
                    command = json.loads(line)
                    if command.get("op") == "advance":
                        answer = await self.advance(command.get("milliseconds"))
                    elif command.get("op") == "clock_wait":
                        answer = await self.clock_wait(command.get("name"), command.get("after_ms"))
                    else:
                        answer = self.request(command)
                        if command.get("op") == "clock_register" and answer.get("ok"):
                            registered = command["name"]
                            self.clock_owners[registered] = writer
                        if command.get("op") in ("sample", "sample_all"):
                            async with self.samples:
                                if command["op"] == "sample":
                                    self.sampled[(command["board"], command["output"])] = self.tick
                                else:
                                    for index in range(4):
                                        self.sampled[(command["board"], index)] = self.tick
                                self.samples.notify_all()
                        if command.get("op") == "clock_ack":
                            async with self.samples:
                                self.samples.notify_all()
                except (KeyError, TypeError, ValueError) as error:
                    answer = {"ok": False, "error": str(error)}
                writer.write((json.dumps(answer, separators=(",", ":")) + "\n").encode())
                await writer.drain()
        except (ConnectionError, asyncio.LimitOverrunError):
            pass
        finally:
            if registered is not None and self.clock_owners.get(registered) is writer:
                async with self.samples:
                    self.clock_acks.pop(registered, None)
                    self.clock_owners.pop(registered, None)
                    self.samples.notify_all()
            writer.close()
            try:
                await writer.wait_closed()
            except ConnectionError:
                pass


def split_address(address: str) -> tuple[str, int]:
    host, port = address.rsplit(":", 1)
    return host, int(port)


async def run(fixture: Path, signals: str, listen: str) -> None:
    service = WorldService(World(json.loads(fixture.read_text())))
    server = await asyncio.start_server(service.client, *split_address(listen))
    LOG.info("physical world listening on %s", listen)
    task = asyncio.create_task(service.signals(signals))
    try:
        async with server:
            await server.serve_forever()
    finally:
        task.cancel()


def main() -> None:
    parser = argparse.ArgumentParser(description="ARC physical test fixture")
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--signals", default="127.0.0.1:17501")
    parser.add_argument("--listen", default="127.0.0.1:17502")
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(message)s")
    asyncio.run(run(args.fixture, args.signals, args.listen))


if __name__ == "__main__":
    main()
