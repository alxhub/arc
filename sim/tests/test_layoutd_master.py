"""Real layoutd + MQTT + host firmware, with portable CAN/signal buses.

Requires Docker and ARC_LAYOUTD_BIN, ARC_PSU_BIN, ARC_DST_REV1_BIN,
ARC_DST_REV2_BIN. The test owns and removes its isolated MQTT container.
"""
import asyncio
import json
import os
import socket
import unittest
import uuid

from sim.canlab.bus import CanBus
from sim.linklab.bus import LinkBus
from sim.world.model import World
from sim.world.service import WorldService
from pathlib import Path


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


@unittest.skipUnless(all(os.getenv(k) for k in (
    "ARC_LAYOUTD_BIN", "ARC_PSU_BIN", "ARC_DST_REV1_BIN", "ARC_DST_REV2_BIN")),
    "requires layoutd and all host firmware binaries")
class LayoutMasterTests(unittest.IsolatedAsyncioTestCase):
    async def test_good_layout_assigns_one_master_and_permission_survives_daemon_exit(self):
        name = "arc-master-test-" + uuid.uuid4().hex[:10]
        mqtt_port = free_port()
        signals = LinkBus()
        signal_port = await signals.start("127.0.0.1", 0)
        fixture = Path(__file__).resolve().parents[1] / "world/fixtures/two_districts.json"
        world = WorldService(World(json.loads(fixture.read_text())))
        world_server = await asyncio.start_server(world.client, "127.0.0.1", 0)
        world_port = world_server.sockets[0].getsockname()[1]
        signal_task = asyncio.create_task(world.signals(f"127.0.0.1:{signal_port}"))
        can = CanBus(clock=f"127.0.0.1:{world_port}")
        can_port = await can.start("127.0.0.1", 0)
        processes = []

        async def command(*args):
            proc = await asyncio.create_subprocess_exec(*args, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
            out, err = await asyncio.wait_for(proc.communicate(), 60)
            self.assertEqual(proc.returncode, 0, err.decode())
            return out.decode()

        async def publish(topic, value):
            await command("docker", "exec", name, "mosquitto_pub", "-t", topic, "-m", json.dumps(value), "-r", "-q", "1")

        async def control(port):
            reader, writer = await asyncio.open_connection("127.0.0.1", port)
            try:
                writer.write(b'{"op":"status"}\n')
                await writer.drain()
                return json.loads(await asyncio.wait_for(reader.readline(), 2))
            finally:
                writer.close()
                await writer.wait_closed()

        async def wait_status(port, predicate):
            last = None
            for _ in range(200):
                try:
                    last = await control(port)
                    if predicate(last): return last
                except ConnectionRefusedError:
                    pass
                await world.advance(10)
            self.fail(f"timed out waiting for node: {last}")

        async def wait_participants(expected):
            for _ in range(200):
                if expected <= set(world.clock_acks):
                    return
                await asyncio.sleep(0.05)
            self.fail(f"missing clock participants: {expected - set(world.clock_acks)}")

        async def wait_absent(name):
            for _ in range(200):
                if name not in world.clock_acks:
                    return
                await asyncio.sleep(0.01)
            self.fail(f"clock participant {name} did not disconnect")

        async def node(binary, uid, port, name):
            proc = await asyncio.create_subprocess_exec(os.environ[binary], "--uid", uid,
                "--bus", f"127.0.0.1:{can_port}", "--signals", f"127.0.0.1:{signal_port}",
                "--clock", f"127.0.0.1:{world_port}", "--name", name,
                "--control", f"127.0.0.1:{port}", stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.PIPE)
            processes.append(proc)
            return proc

        def network_id(uid):
            value = 0x811c9dc5
            for byte in b"stm32" + bytes.fromhex(uid):
                value = ((value ^ byte) * 0x01000193) & 0xffffffff
            return f"{value & 0xffffff:06x}"

        try:
            await command("docker", "run", "-d", "--rm", "--name", name,
                "-p", f"127.0.0.1:{mqtt_port}:1883", "eclipse-mosquitto:2",
                "mosquitto", "-c", "/mosquitto-no-auth.conf")
            uids = ["303132333435363738393a3b", "000102030405060708090a0b", "101112131415161718191a1b"]
            ids = [network_id(uid) for uid in uids]
            ports = [free_port() for _ in uids]
            await node("ARC_PSU_BIN", uids[0], ports[0], "psu")
            await node("ARC_DST_REV1_BIN", uids[1], ports[1], "node_a")
            await wait_participants({"can", "psu", "node_a"})
            await world.advance(21)
            await wait_status(ports[0], lambda s: s["state"] == "online")
            self.assertFalse((await control(ports[0]))["dcc_enabled"])
            self.assertFalse((await wait_status(ports[1], lambda s: True))["sync_enabled"])
            for index, id in enumerate(ids):
                await publish(f"/test/layout/node/n{index}/fact", {
                    "kind": "infrastructure" if index == 0 else "district",
                    "topology": {"status": "configured", "connections": []},
                    "binding": {"node_id": id, "output": 0}})
            await publish("/test/layout/index", ["n0", "n1", "n2"])
            daemon = await asyncio.create_subprocess_exec(os.environ["ARC_LAYOUTD_BIN"], "test",
                f"tcp://127.0.0.1:{can_port}", "127.0.0.1", str(mqtt_port),
                stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.PIPE,
                env={**os.environ, "ARC_SIM_CLOCK": f"127.0.0.1:{world_port}"})
            processes.append(daemon)
            # Wait past Presence cadence with an expected board absent: no grant.
            await wait_participants({"layoutd"})
            await world.advance(5_500)
            self.assertIsNone(daemon.returncode)
            self.assertFalse((await control(ports[0]))["dcc_permitted"])
            self.assertFalse((await control(ports[1]))["sync_permitted"])
            await node("ARC_DST_REV2_BIN", uids[2], ports[2], "node_b")
            await wait_participants({"node_b"})
            await wait_status(ports[0], lambda s: s["dcc_enabled"])
            self.assertFalse((await control(ports[1]))["sync_permitted"])
            self.assertFalse((await control(ports[2]))["sync_permitted"])
            selected = await command("docker", "exec", name, "mosquitto_sub", "-t", "/test/layout/dcc_master", "-C", "1", "-W", "3")
            self.assertEqual(json.loads(selected), ids[0])
            daemon.terminate()
            await daemon.wait()
            await wait_absent("layoutd")
            self.assertTrue((await control(ports[0]))["dcc_enabled"])
            # Restarting the source clears permission even though the layout was good.
            processes[0].terminate()
            await processes[0].wait()
            await wait_absent("psu")
            await node("ARC_PSU_BIN", uids[0], ports[0], "psu")
            await wait_participants({"psu"})
            status = await wait_status(ports[0], lambda s: s["state"] == "online")
            self.assertFalse(status["dcc_permitted"])
            self.assertFalse(status["dcc_enabled"])
        finally:
            for proc in processes:
                if proc.returncode is None: proc.terminate()
                await proc.wait()
            cleanup = await asyncio.create_subprocess_exec("docker", "rm", "-f", name,
                stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.DEVNULL)
            await cleanup.wait()
            await can.close()
            signal_task.cancel()
            try:
                await signal_task
            except asyncio.CancelledError:
                pass
            await signals.close()
            world_server.close()
            await world_server.wait_closed()
