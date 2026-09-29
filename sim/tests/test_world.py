import asyncio
import json
from pathlib import Path
import unittest

from sim.world.model import World
from sim.world.service import WorldService


FIXTURE = Path(__file__).resolve().parents[1] / "world/fixtures/two_districts.json"


def packet(*data: int) -> str:
    checksum = 0
    for byte in data:
        checksum ^= byte
    return "1" * 14 + "".join("0" + f"{byte:08b}" for byte in (*data, checksum)) + "1"


class WorldTests(unittest.TestCase):
    def setUp(self):
        self.fixture = json.loads(FIXTURE.read_text())
        self.world = World(self.fixture)
        self.west = ("fa5138", 0)
        self.east = ("fa5138", 1)

    def power(self):
        self.world.link_power = True
        self.world.last_packet_s = 0
        for feed in (self.west, self.east):
            self.world.set_output(feed, True, "synced")

    def test_packet_reaches_only_powered_loco_and_motion_crosses_district(self):
        self.world.receive_packet(packet(3, 0x3f, 0xff))
        self.assertEqual(self.world.locos["test"].speed, 0)
        self.power()
        self.world.receive_packet(packet(3, 0x3f, 0xff))
        self.assertEqual(self.world.locos["test"].speed, 126)
        self.assertEqual(self.world.load(self.west), (80, False))
        self.assertEqual(self.world.load(self.east), (0, False))
        self.world.step(0.01)
        self.assertGreater(self.world.locos["test"].offset_mm, 30)
        # Continuous DCC packets keep the source live during the journey.
        for _ in range(180):
            self.world.receive_packet(packet(3, 0x3f, 0xff))
            self.world.step(0.01)
        self.assertEqual(self.world.locos["test"].piece, "east")
        self.assertGreater(self.world.load(self.east)[0], 0)
        self.assertEqual(self.world.load(self.west), (0, False))

    def test_pickup_bridge_trips_both_outputs_for_mismatched_phase(self):
        self.power()
        self.world.locos["test"].offset_mm = 196
        self.assertEqual(self.world.load(self.west), (80, False))
        self.assertEqual(self.world.load(self.east), (80, False))
        self.world.set_output(self.east, True, "synced", phase=1)
        self.assertEqual(self.world.load(self.west), (5_000, True))
        self.assertEqual(self.world.load(self.east), (5_000, True))
        self.world.set_output(self.east, False, "synced", phase=1)
        self.assertEqual(self.world.load(self.west), (80, False))
        self.assertEqual(self.world.load(self.east), (0, False))

    def test_swapped_rails_require_opposite_phase(self):
        self.fixture["connections"][0]["rails"] = "swapped"
        world = World(self.fixture)
        world.link_power = True
        world.last_packet_s = 0
        world.locos["test"].offset_mm = 196
        world.set_output(self.west, True, "synced")
        world.set_output(self.east, True, "synced")
        self.assertEqual(world.load(self.west), (5_000, True))
        world.set_output(self.east, True, "synced", phase=1)
        self.assertEqual(world.load(self.west), (80, False))

    def test_decoder_direction_reverses_and_power_loss_stops(self):
        self.power()
        loco = self.world.locos["test"]
        self.world.receive_packet(packet(3, 0x3f, 0x7f))
        self.world.step(0.01)
        self.assertLess(loco.offset_mm, 30)
        stopped_at = loco.offset_mm
        self.world.link_power = False
        self.world.step(0.01)
        self.assertEqual(loco.offset_mm, stopped_at)
        self.assertEqual(self.world.load(self.west), (0, False))

    def test_turnout_selects_entry_path(self):
        self.fixture["pieces"]["east"] = {
            "feed": {"board": "fa5138", "output": 1},
            "selected": "diverging",
            "paths": {
                "straight": {"from": "west", "to": "east", "length_mm": 200},
                "diverging": {"from": "west", "to": "branch", "length_mm": 230},
            },
        }
        self.world = World(self.fixture)
        self.power()
        loco = self.world.locos["test"]
        loco.offset_mm = 199
        loco.speed = 126
        self.world.step(0.02)
        self.assertEqual((loco.piece, loco.path), ("east", "diverging"))
        self.assertEqual(loco.offset_mm, 1)


class ClockTests(unittest.IsolatedAsyncioTestCase):
    async def test_host_tick_waits_for_signal_delivery_and_acknowledgements(self):
        service = WorldService(World(json.loads(FIXTURE.read_text())))
        service.request({"op": "clock_register", "name": "psu"})
        service.request({"op": "clock_register", "name": "node_a"})
        advance = asyncio.create_task(service.advance(1))
        node_tick = asyncio.create_task(service.clock_wait("node_a", 0))
        self.assertEqual((await service.clock_wait("psu", 0))["time_ms"], 1)
        self.assertFalse(node_tick.done())
        service.request({"op": "clock_ack", "name": "psu", "time_ms": 1})
        async with service.samples:
            service.samples.notify_all()
        await asyncio.sleep(0)
        self.assertFalse(node_tick.done())
        async with service.samples:
            service.signal_tick = 1
            service.samples.notify_all()
        self.assertEqual((await node_tick)["time_ms"], 1)
        async with service.samples:
            service.source_barriers["node_a"] = 1
            service.samples.notify_all()
        service.request({"op": "clock_ack", "name": "node_a", "time_ms": 1})
        async with service.samples:
            service.samples.notify_all()
        self.assertEqual((await advance)["time_ms"], 1)

    async def test_world_time_advances_only_by_explicit_command(self):
        service = WorldService(World(json.loads(FIXTURE.read_text())))
        self.assertEqual(service.request({"op": "status"})["time_ms"], 0)
        service.world.link_power = True
        service.world.last_packet_s = 0
        for output in (0, 1):
            service.request({"op": "sample", "board": "fa5138", "output": output,
                             "enabled": True, "mode": "synced"})
        service.world.receive_packet(packet(3, 0x3f, 0xff))
        before = service.request({"op": "status"})
        self.assertEqual(before["time_ms"], 0)
        self.assertEqual(before["locos"]["test"]["offset_mm"], 30)
        await service.advance(100)
        after = service.request({"op": "status"})
        self.assertEqual(after["time_ms"], 100)
        self.assertEqual(after["locos"]["test"]["offset_mm"], 40)
        self.assertEqual(service.request({"op": "status"}), after)
        with self.assertRaises(ValueError):
            await service.advance(-1)

    async def test_advance_waits_for_registered_output_sample(self):
        service = WorldService(World(json.loads(FIXTURE.read_text())))
        feed = ("fa5138", 0)
        service.sampled[feed] = 0
        advance = asyncio.create_task(service.advance(1))
        await asyncio.sleep(0)
        self.assertEqual(service.tick, 1)
        self.assertFalse(advance.done())
        async with service.samples:
            service.sampled[feed] = 1
            service.samples.notify_all()
        self.assertEqual((await advance)["time_ms"], 1)

    async def test_batched_sample_sees_both_sides_of_bridge(self):
        service = WorldService(World(json.loads(FIXTURE.read_text())))
        service.world.link_power = True
        service.world.last_packet_s = 0
        service.world.locos["test"].offset_mm = 196
        outputs = [{"enabled": True, "mode": "synced", "phase": index == 1}
                   for index in range(4)]
        response = service.request({"op": "sample_all", "board": "fa5138", "outputs": outputs})
        self.assertEqual(response["samples"][:2],
                         [{"current_ma": 5_000, "fault": True}] * 2)


if __name__ == "__main__":
    unittest.main()
