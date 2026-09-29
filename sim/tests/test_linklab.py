import asyncio
import json
import unittest

from sim.linklab.bus import LinkBus, validate


class SignalTests(unittest.TestCase):
    def test_rejects_malformed_dcc_timing(self):
        bits = "1" * 42
        self.assertEqual(validate(json.dumps({"kind": "dcc_packet", "bits": bits,
                                              "half_us": [58] * 42}).encode())["bits"], bits)
        with self.assertRaises(ValueError):
            validate(json.dumps({"kind": "dcc_packet", "bits": bits,
                                 "half_us": [58] * 41}).encode())
        speed_bits = "1" * 14 + "0" + "0" * 8 + "0" + "0" * 8 + "0" + "0" * 8 + "0" + "0" * 8 + "1"
        self.assertEqual(len(speed_bits), 51)
        self.assertEqual(validate(json.dumps({"kind": "dcc_packet", "bits": speed_bits,
                                              "half_us": [58] * 51}).encode())["bits"], speed_bits)


class LinkBusTests(unittest.IsolatedAsyncioTestCase):
    async def test_power_replays_to_late_node_and_packets_broadcast(self):
        bus = LinkBus()
        port = await bus.start("127.0.0.1", 0)
        try:
            a_reader, a_writer = await asyncio.open_connection("127.0.0.1", port)
            power = {"kind": "power", "enabled": True, "voltage_mv": 15000, "current_ma": 250}
            a_writer.write((json.dumps(power) + "\n").encode())
            await a_writer.drain()
            self.assertEqual(json.loads(await asyncio.wait_for(a_reader.readline(), 1)), power)
            b_reader, b_writer = await asyncio.open_connection("127.0.0.1", port)
            self.assertEqual(json.loads(await asyncio.wait_for(b_reader.readline(), 1)), power)
            packet = {"kind": "dcc_packet", "bits": "1" * 42, "half_us": [58] * 42}
            a_writer.write((json.dumps(packet) + "\n").encode())
            await a_writer.drain()
            for reader in (a_reader, b_reader):
                self.assertEqual(json.loads(await asyncio.wait_for(reader.readline(), 1)), packet)
            barrier = {"kind": "barrier", "time_ms": 7}
            a_writer.write((json.dumps(barrier) + "\n").encode())
            await a_writer.drain()
            for reader in (a_reader, b_reader):
                self.assertEqual(json.loads(await asyncio.wait_for(reader.readline(), 1)), barrier)
            self.assertEqual(json.loads(await asyncio.wait_for(a_reader.readline(), 1)),
                             {"kind": "barrier_ack", "time_ms": 7})
            a_writer.close()
            b_writer.close()
            await asyncio.gather(a_writer.wait_closed(), b_writer.wait_closed())
        finally:
            await bus.close()
