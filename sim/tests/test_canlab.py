import asyncio
import json
from pathlib import Path
import unittest

from sim.canlab.bus import CanBus
from sim.canlab.frame import Frame
from sim.world.model import World
from sim.world.service import WorldService


class FrameTests(unittest.TestCase):
    def test_round_trip_and_reject_invalid_dlc(self):
        frame = Frame(0x1FFA5138, bytes(range(16)))
        self.assertEqual(Frame.from_line(frame.to_line()), frame)
        with self.assertRaises(ValueError):
            Frame(0x1FFA5138, bytes(range(9)))


class BusTests(unittest.IsolatedAsyncioTestCase):
    async def test_clocked_batch_waits_for_five_ms_and_arbitrates(self):
        fixture = Path(__file__).resolve().parents[1] / "world/fixtures/two_districts.json"
        world = WorldService(World(json.loads(fixture.read_text())))
        clock_server = await asyncio.start_server(world.client, "127.0.0.1", 0)
        clock_port = clock_server.sockets[0].getsockname()[1]
        bus = CanBus(clock=f"127.0.0.1:{clock_port}")
        bus_port = await bus.start("127.0.0.1", 0)
        try:
            for _ in range(100):
                if "can" in world.clock_acks:
                    break
                await asyncio.sleep(0.001)
            self.assertIn("can", world.clock_acks)
            reader, sender = await asyncio.open_connection("127.0.0.1", bus_port)
            observer, peer = await asyncio.open_connection("127.0.0.1", bus_port)
            high = Frame(0x1FFA5138, b"\x01")
            low = Frame(0x01000001, b"\x02")
            sender.write(high.to_line() + low.to_line() + b'{"kind":"barrier"}\n')
            await sender.drain()
            self.assertEqual(json.loads(await reader.readline()), {"kind": "barrier", "ok": True})
            self.assertEqual((await world.advance(4))["time_ms"], 4)
            with self.assertRaises(asyncio.TimeoutError):
                await asyncio.wait_for(observer.readline(), 0.01)
            self.assertEqual((await world.advance(1))["time_ms"], 5)
            self.assertEqual(Frame.from_line(await asyncio.wait_for(observer.readline(), 1)), low)
            self.assertEqual(Frame.from_line(await asyncio.wait_for(observer.readline(), 1)), high)
            sender.close()
            peer.close()
            await asyncio.gather(sender.wait_closed(), peer.wait_closed())
        finally:
            await bus.close()
            clock_server.close()
            await clock_server.wait_closed()


if __name__ == "__main__":
    unittest.main()
