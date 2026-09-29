"""Host-process transmitter test; set ARC_DST_REV1_BIN and ARC_DST_REV2_BIN.

The signal bus supplies power only, so every observed DCC packet comes from DST.
"""

import asyncio
import json
import os
import socket
import unittest
from pathlib import Path

from sim.canlab.bus import CanBus
from sim.linklab.bus import LinkBus
from sim.world.model import World
from sim.world.service import WorldService


@unittest.skipUnless(os.getenv("ARC_DST_REV1_BIN") and os.getenv("ARC_DST_REV2_BIN"),
                     "requires both DST host binaries")
class DstSyncTests(unittest.IsolatedAsyncioTestCase):
    async def test_rev1_requires_can_grant_and_retains_it_until_restart(self):
        signals = LinkBus()
        signal_port = await signals.start("127.0.0.1", 0)
        fixture = Path(__file__).resolve().parents[1] / "world/fixtures/two_districts.json"
        world = WorldService(World(json.loads(fixture.read_text())))
        world_server = await asyncio.start_server(world.client, "127.0.0.1", 0)
        world_port = world_server.sockets[0].getsockname()[1]
        world_signals = asyncio.create_task(world.signals(f"127.0.0.1:{signal_port}"))
        can = CanBus(clock=f"127.0.0.1:{world_port}")
        can_port = await can.start("127.0.0.1", 0)
        reader, writer = await asyncio.open_connection("127.0.0.1", signal_port)
        processes = []
        can_reader, can_writer = await asyncio.open_connection("127.0.0.1", can_port)

        async def fake_psu_clock():
            clock_reader, clock_writer = await asyncio.open_connection("127.0.0.1", world_port)
            signal_reader, signal_writer = await asyncio.open_connection("127.0.0.1", signal_port)
            async def request(command):
                clock_writer.write((json.dumps(command) + "\n").encode())
                await clock_writer.drain()
                result = json.loads(await clock_reader.readline())
                if not result["ok"]:
                    raise AssertionError(result)
                return result
            try:
                now = (await request({"op": "clock_register", "name": "psu"}))["time_ms"]
                while True:
                    tick = await request({"op": "clock_wait", "name": "psu", "after_ms": now})
                    if tick.get("pending"):
                        continue
                    now = tick["time_ms"]
                    signal_writer.write((json.dumps({"kind": "barrier", "time_ms": now}) + "\n").encode())
                    await signal_writer.drain()
                    while True:
                        signal = json.loads(await signal_reader.readline())
                        if signal.get("kind") == "barrier_ack" and signal.get("time_ms") == now:
                            break
                    await request({"op": "clock_ack", "name": "psu", "time_ms": now})
            finally:
                signal_writer.close()
                clock_writer.close()
                await signal_writer.wait_closed()
                await clock_writer.wait_closed()

        psu_task = asyncio.create_task(fake_psu_clock())

        async def wait_participants(expected):
            for _ in range(200):
                if expected <= set(world.clock_acks):
                    return
                await asyncio.sleep(0.01)
            self.fail(f"missing clock participants: {expected - set(world.clock_acks)}")

        async def grant(target, malformed=False):
            payload = "0101" + target + ("00" if malformed else "")
            can_writer.write((json.dumps({"id": 0x14000001, "data": payload}) + "\n" + '{"kind":"barrier"}\n').encode())
            await can_writer.drain()
            while json.loads(await can_reader.readline()).get("kind") != "barrier":
                pass
            await world.advance(10)

        async def control(port, **command):
            r, w = await asyncio.open_connection("127.0.0.1", port)
            try:
                w.write((json.dumps(command) + "\n").encode())
                await w.drain()
                return json.loads(await asyncio.wait_for(r.readline(), 2))
            finally:
                w.close()
                await w.wait_closed()

        async def power(enabled):
            writer.write((json.dumps({"kind": "power", "enabled": enabled,
                                      "voltage_mv": 15000, "current_ma": 0}) + "\n" +
                          json.dumps({"kind": "barrier", "time_ms": world.tick, "source": "test"}) + "\n").encode())
            await writer.drain()
            while True:
                signal = json.loads(await reader.readline())
                if signal.get("kind") == "barrier_ack" and signal.get("time_ms") == world.tick:
                    break
            await world.advance(1)
            for _ in range(100):
                states = [await control(p, op="status") for p in ports]
                if all(s["link_power"] == enabled for s in states):
                    return states
                await world.advance(1)
            self.fail("nodes did not observe power change")

        async def packet():
            while True:
                value = json.loads(await asyncio.wait_for(reader.readline(), 1))
                if value["kind"] == "dcc_packet":
                    return value

        try:
            ports = []
            ids = []
            for rev in (1, 2):
                with socket.socket() as sock:
                    sock.bind(("127.0.0.1", 0))
                    port = sock.getsockname()[1]
                ports.append(port)
                proc = await asyncio.create_subprocess_exec(
                    os.environ[f"ARC_DST_REV{rev}_BIN"],
                    "--uid", f"{rev:024x}", "--bus", f"127.0.0.1:{can_port}",
                    "--signals", f"127.0.0.1:{signal_port}",
                    "--clock", f"127.0.0.1:{world_port}", "--name", "node_a" if rev == 1 else "node_b",
                    "--control", f"127.0.0.1:{port}", "--source-ready", "true",
                    stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.PIPE)
                processes.append(proc)
                for _ in range(100):
                    try:
                        status = await control(port, op="status")
                        break
                    except ConnectionRefusedError:
                        await asyncio.sleep(0.01)
                else:
                    self.fail(f"rev{rev} failed to start")
                ids.append(status["network_id"])
                self.assertEqual(status["sync_capable"], rev == 1)
                self.assertFalse(status["sync_enabled"])
                self.assertFalse(status["sync_permitted"])

            await wait_participants({"can", "psu", "node_a", "node_b"})
            await power(True)
            with self.assertRaises(asyncio.TimeoutError):
                await asyncio.wait_for(packet(), 0.05)
            await grant(ids[1])
            await grant(ids[0], malformed=True)
            self.assertFalse((await control(ports[0], op="status"))["sync_permitted"])
            self.assertFalse((await control(ports[1], op="status"))["sync_permitted"])
            await grant(ids[0])
            await grant(ids[0])
            await world.advance(30)
            expected = "1" * 14 + "0" + "11111111" + "0" + "00000000" + "0" + "11111111" + "1"
            for _ in range(3):
                frame = await packet()
                self.assertEqual(frame["bits"], expected)
                self.assertEqual(frame["half_us"], [58 if b == "1" else 100 for b in expected])
            for port in ports:
                status = await control(port, op="status")
                self.assertTrue(status["source_ready"])
                self.assertTrue(all(not d["enabled"] for d in status["districts"]))

            # The selected district source also accepts the shared throttle format.
            throttle = (bytes.fromhex("0101" + ids[0]) + (1).to_bytes(4, "big")
                        + (1).to_bytes(4, "big") + bytes([0, 0, 3, 1, 20, 0, 0]))
            can_writer.write((json.dumps({"id": 0x08000001, "data": throttle.hex()}) + "\n" + '{"kind":"barrier"}\n').encode())
            await can_writer.drain()
            while json.loads(await can_reader.readline()).get("kind") != "barrier":
                pass
            await world.advance(200)
            for _ in range(30):
                frame = await packet()
                bits = frame["bits"]
                decoded = bytes(int(bits[i + 1:i + 9], 2) for i in range(14, len(bits) - 1, 9))
                if decoded == bytes([3, 0x3f, 0x95, 0xa9]): break
            else:
                self.fail("DST did not emit the commanded speed packet")

            await power(False)
            status = await control(ports[0], op="status")
            self.assertFalse(status["sync_enabled"])
            self.assertTrue(status["sync_permitted"])
            for port in ports:
                self.assertFalse((await control(port, op="status"))["source_ready"])
            await power(True)
            await world.advance(20)
            self.assertTrue((await control(ports[0], op="status"))["sync_enabled"])
            self.assertTrue((await control(ports[1], op="status"))["source_ready"])
        finally:
            for proc in processes:
                if proc.returncode is None:
                    proc.terminate()
                await proc.wait()
            can_writer.close()
            await can_writer.wait_closed()
            writer.close()
            await writer.wait_closed()
            await can.close()
            psu_task.cancel()
            try:
                await psu_task
            except asyncio.CancelledError:
                pass
            world_signals.cancel()
            try:
                await world_signals
            except asyncio.CancelledError:
                pass
            await signals.close()
            world_server.close()
            await world_server.wait_closed()
