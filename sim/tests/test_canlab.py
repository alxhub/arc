import asyncio
import unittest

from sim.canlab.bus import CanBus
from sim.canlab.frame import Frame


class FrameTests(unittest.TestCase):
    def test_round_trip_and_reject_invalid_dlc(self):
        frame = Frame(0x1FFA5138, bytes(range(16)))
        self.assertEqual(Frame.from_line(frame.to_line()), frame)
        with self.assertRaises(ValueError):
            Frame(0x1FFA5138, bytes(range(9)))


class BusTests(unittest.IsolatedAsyncioTestCase):
    async def test_broadcasts_to_all_nodes_in_arbitration_order(self):
        bus = CanBus(tick_seconds=0.02)
        port = await bus.start("127.0.0.1", 0)
        try:
            a_reader, a_writer = await asyncio.open_connection("127.0.0.1", port)
            b_reader, b_writer = await asyncio.open_connection("127.0.0.1", port)
            high = Frame(0x01000001, b"\x01")
            low = Frame(0x1FFA5138, bytes(range(16)))
            a_writer.write(low.to_line())
            b_writer.write(high.to_line())
            await asyncio.gather(a_writer.drain(), b_writer.drain())
            for reader in (a_reader, b_reader):
                first = Frame.from_line(await asyncio.wait_for(reader.readline(), 1))
                second = Frame.from_line(await asyncio.wait_for(reader.readline(), 1))
                self.assertEqual((first, second), (high, low))
            a_writer.close()
            b_writer.close()
            await asyncio.gather(a_writer.wait_closed(), b_writer.wait_closed())
        finally:
            await bus.close()


if __name__ == "__main__":
    unittest.main()
