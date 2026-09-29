"""Broadcast virtual CAN FD frames over a portable TCP transport.

The TCP framing is for the simulator only: one JSON object per line with an
extended CAN identifier and hexadecimal data bytes. It is not an ARC wire format.
"""

import argparse
from contextlib import suppress
import asyncio
import json
import logging

from .frame import Frame

LOG = logging.getLogger("canlab.bus")


class CanBus:
    def __init__(self, clock: str) -> None:
        self.clock = clock
        self.clients: set[asyncio.StreamWriter] = set()
        self.frames: asyncio.Queue[Frame] = asyncio.Queue()
        self.server: asyncio.Server | None = None
        self.dispatch_task: asyncio.Task[None] | None = None

    async def start(self, host: str, port: int) -> int:
        self.server = await asyncio.start_server(self._client, host, port)
        self.dispatch_task = asyncio.create_task(self._clock_dispatch())
        return self.server.sockets[0].getsockname()[1]

    async def close(self) -> None:
        if self.server is not None:
            self.server.close()
        if self.dispatch_task is not None:
            self.dispatch_task.cancel()
            try:
                await self.dispatch_task
            except asyncio.CancelledError:
                pass
        for writer in tuple(self.clients):
            writer.close()
            with suppress(ConnectionError):
                await writer.wait_closed()
        if self.server is not None:
            await self.server.wait_closed()

    async def _client(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        self.clients.add(writer)
        try:
            while line := await reader.readline():
                try:
                    if len(line) > 512:
                        raise ValueError("frame exceeds simulator line limit")
                    value = json.loads(line)
                    if isinstance(value, dict) and value.get("kind") == "barrier":
                        writer.write(b'{"kind":"barrier","ok":true}\n')
                        await writer.drain()
                        continue
                    frame = Frame.from_line(line)
                except (ValueError, KeyError, TypeError) as error:
                    LOG.warning("discarded invalid frame: %s", error)
                    continue
                await self.frames.put(frame)
        except ConnectionError:
            pass
        finally:
            self.clients.discard(writer)
            writer.close()
            with suppress(ConnectionError):
                await writer.wait_closed()

    async def _broadcast(self, batch: list[Frame]) -> None:
        for frame in sorted(batch, key=lambda item: item.can_id):
            LOG.info("CAN %08x %s", frame.can_id, frame.data.hex(" "))
            await self._broadcast_line(frame.to_line())

    async def _broadcast_line(self, line: bytes) -> None:
        for writer in tuple(self.clients):
            writer.write(line)
        for writer in tuple(self.clients):
            try:
                await asyncio.wait_for(writer.drain(), timeout=1)
            except (ConnectionError, asyncio.TimeoutError):
                self.clients.discard(writer)
                writer.close()

    async def _clock_dispatch(self) -> None:
        host, port = self.clock.rsplit(":", 1)
        while True:
            try:
                reader, writer = await asyncio.open_connection(host, int(port))
                try:
                    async def request(command: dict) -> dict:
                        writer.write((json.dumps(command, separators=(",", ":")) + "\n").encode())
                        await writer.drain()
                        answer = json.loads(await reader.readline())
                        if not answer.get("ok"):
                            raise ValueError(f"clock rejected {command}: {answer}")
                        return answer

                    now = (await request({"op": "clock_register", "name": "can"}))["time_ms"]
                    while True:
                        clock = await request({"op": "clock_wait", "name": "can", "after_ms": now})
                        if clock.get("pending"):
                            continue
                        now = clock["time_ms"]
                        batch = []
                        while not self.frames.empty():
                            batch.append(self.frames.get_nowait())
                        await self._broadcast(batch)
                        await self._broadcast_line((json.dumps({"kind": "can_tick", "time_ms": now}) + "\n").encode())
                        await request({"op": "clock_ack", "name": "can", "time_ms": now})
                finally:
                    writer.close()
                    await writer.wait_closed()
            except (OSError, ValueError, json.JSONDecodeError) as error:
                LOG.warning("CAN clock connection: %s", error)
                await asyncio.sleep(0.1)


async def run(host: str, port: int, clock: str) -> None:
    bus = CanBus(clock)
    actual_port = await bus.start(host, port)
    LOG.info("virtual CAN bus listening on %s:%s", host, actual_port)
    try:
        await bus.server.serve_forever()
    finally:
        await bus.close()


def main() -> None:
    parser = argparse.ArgumentParser(description="ARC virtual CAN bus")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=17500)
    parser.add_argument("--clock", required=True)
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(message)s")
    asyncio.run(run(args.host, args.port, args.clock))


if __name__ == "__main__":
    main()
