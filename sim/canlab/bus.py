"""Broadcast virtual CAN FD frames over a portable TCP transport.

The TCP framing is for the simulator only: one JSON object per line with an
extended CAN identifier and hexadecimal data bytes. It is not an ARC wire format.
"""

import argparse
import asyncio
import logging

from .frame import Frame

LOG = logging.getLogger("canlab.bus")


class CanBus:
    def __init__(self, tick_seconds: float = 0.005) -> None:
        self.tick_seconds = tick_seconds
        self.clients: set[asyncio.StreamWriter] = set()
        self.frames: asyncio.Queue[Frame] = asyncio.Queue()
        self.server: asyncio.Server | None = None
        self.dispatch_task: asyncio.Task[None] | None = None

    async def start(self, host: str, port: int) -> int:
        self.server = await asyncio.start_server(self._client, host, port)
        self.dispatch_task = asyncio.create_task(self._dispatch())
        return self.server.sockets[0].getsockname()[1]

    async def close(self) -> None:
        if self.server is not None:
            self.server.close()
            await self.server.wait_closed()
        if self.dispatch_task is not None:
            self.dispatch_task.cancel()
            try:
                await self.dispatch_task
            except asyncio.CancelledError:
                pass
        for writer in tuple(self.clients):
            writer.close()
            await writer.wait_closed()

    async def _client(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        self.clients.add(writer)
        try:
            while line := await reader.readline():
                try:
                    if len(line) > 512:
                        raise ValueError("frame exceeds simulator line limit")
                    frame = Frame.from_line(line)
                except (ValueError, KeyError, TypeError) as error:
                    LOG.warning("discarded invalid frame: %s", error)
                    continue
                await self.frames.put(frame)
        finally:
            self.clients.discard(writer)
            writer.close()
            await writer.wait_closed()

    async def _dispatch(self) -> None:
        while True:
            batch = [await self.frames.get()]
            await asyncio.sleep(self.tick_seconds)
            while not self.frames.empty():
                batch.append(self.frames.get_nowait())
            for frame in sorted(batch, key=lambda item: item.can_id):
                LOG.info("CAN %08x %s", frame.can_id, frame.data.hex(" "))
                line = frame.to_line()
                for writer in tuple(self.clients):
                    writer.write(line)
                for writer in tuple(self.clients):
                    try:
                        await asyncio.wait_for(writer.drain(), timeout=1)
                    except (ConnectionError, asyncio.TimeoutError):
                        self.clients.discard(writer)
                        writer.close()


async def run(host: str, port: int, tick_ms: float) -> None:
    bus = CanBus(tick_ms / 1000)
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
    parser.add_argument("--tick-ms", type=float, default=5)
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(message)s")
    asyncio.run(run(args.host, args.port, args.tick_ms))


if __name__ == "__main__":
    main()
