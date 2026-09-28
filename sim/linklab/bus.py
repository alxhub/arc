"""Broadcast simulated link power and DCC packets outside the CAN bus.

The transport carries packet bits and half-bit durations rather than sampling
individual edges. It preserves the information district receivers need to
validate the DCC stream without using wall-clock scheduling for every edge.
"""

import argparse
import asyncio
import json
import logging

LOG = logging.getLogger("linklab.bus")


def validate(line: bytes) -> dict:
    if len(line) > 1024:
        raise ValueError("signal exceeds simulator line limit")
    value = json.loads(line)
    if not isinstance(value, dict):
        raise ValueError("signal must be an object")
    if value.get("kind") == "power":
        if type(value.get("enabled")) is not bool:
            raise ValueError("power requires enabled boolean")
        if type(value.get("voltage_mv")) is not int or type(value.get("current_ma")) is not int:
            raise ValueError("power requires integer measurements")
    elif value.get("kind") == "dcc_packet":
        bits = value.get("bits")
        halves = value.get("half_us")
        if not isinstance(bits, str) or len(bits) not in (42, 51, 60) or any(bit not in "01" for bit in bits):
            raise ValueError("DCC packet requires 42, 51, or 60 bits")
        if not isinstance(halves, list) or len(halves) != len(bits) or any(type(x) is not int or x <= 0 for x in halves):
            raise ValueError("DCC packet requires one positive half-bit duration per bit")
    else:
        raise ValueError("unknown signal kind")
    return value


class LinkBus:
    def __init__(self) -> None:
        self.clients: set[asyncio.StreamWriter] = set()
        self.power: bytes | None = None
        self.server: asyncio.Server | None = None

    async def start(self, host: str, port: int) -> int:
        self.server = await asyncio.start_server(self._client, host, port)
        return self.server.sockets[0].getsockname()[1]

    async def close(self) -> None:
        if self.server is not None:
            self.server.close()
            await self.server.wait_closed()
        for writer in tuple(self.clients):
            writer.close()
            await writer.wait_closed()

    async def _client(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        self.clients.add(writer)
        if self.power is not None:
            writer.write(self.power)
            await writer.drain()
        try:
            while line := await reader.readline():
                try:
                    signal = validate(line)
                except (ValueError, TypeError) as error:
                    LOG.warning("discarded invalid signal: %s", error)
                    continue
                encoded = (json.dumps(signal, separators=(",", ":")) + "\n").encode()
                if signal["kind"] == "power":
                    self.power = encoded
                for peer in tuple(self.clients):
                    peer.write(encoded)
                for peer in tuple(self.clients):
                    try:
                        await asyncio.wait_for(peer.drain(), timeout=1)
                    except (ConnectionError, asyncio.TimeoutError):
                        self.clients.discard(peer)
                        peer.close()
        finally:
            self.clients.discard(writer)
            writer.close()
            await writer.wait_closed()


async def run(host: str, port: int) -> None:
    bus = LinkBus()
    actual_port = await bus.start(host, port)
    LOG.info("virtual ARC-Link signal bus listening on %s:%s", host, actual_port)
    try:
        await bus.server.serve_forever()
    finally:
        await bus.close()


def main() -> None:
    parser = argparse.ArgumentParser(description="ARC-Link power and DCC simulator bus")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=17501)
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(message)s")
    asyncio.run(run(args.host, args.port))


if __name__ == "__main__":
    main()
