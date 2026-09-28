"""Black-box scenarios for host firmware nodes on the Compose CAN lab."""

import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import time
from typing import Callable

ROOT = Path(__file__).resolve().parents[1]
COMPOSE = ROOT / "sim" / "compose.yaml"


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


class Node:
    def __init__(self, name: str, port: int):
        self.name = name
        self.port = port

    def request(self, command: dict) -> dict:
        with socket.create_connection(("127.0.0.1", self.port), timeout=1) as connection:
            connection.settimeout(2)
            connection.sendall((json.dumps(command) + "\n").encode())
            with connection.makefile("r") as reader:
                line = reader.readline()
        if not line:
            raise RuntimeError(f"{self.name} closed its control connection")
        response = json.loads(line)
        if not response.get("ok"):
            raise RuntimeError(f"{self.name} rejected {command}: {response}")
        return response

    def status(self) -> dict:
        return self.request({"op": "status"})

    def short(self, district: int, value: bool) -> None:
        self.request({"op": "set_short", "district": district, "value": value})

    def source_ready(self, value: bool) -> None:
        self.request({"op": "set_source_ready", "value": value})

    def power(self, voltage_mv: int, current_ma: int = 250,
              monitor_available: bool = True) -> None:
        self.request({"op": "set_power", "voltage_mv": voltage_mv,
                      "current_ma": current_ma, "monitor_available": monitor_available})


class SignalObserver:
    def __init__(self, port: int):
        self.connection = socket.create_connection(("127.0.0.1", port), timeout=1)
        self.connection.settimeout(0.5)
        self.pending = b""

    def close(self) -> None:
        self.connection.close()

    def next(self, kind: str, condition: Callable[[dict], bool], timeout: float = 4) -> dict:
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            while b"\n" not in self.pending:
                try:
                    self.pending += self.connection.recv(4096)
                except socket.timeout:
                    break
            if b"\n" not in self.pending:
                continue
            line, self.pending = self.pending.split(b"\n", 1)
            signal = json.loads(line)
            if signal.get("kind") == kind:
                last = signal
                if condition(signal):
                    return signal
        raise AssertionError(f"timed out waiting for {kind}; last={last}")


def configure(bus_port: int, target: str, district: int, mode: int) -> None:
    """Send the actual ARC CAN FD district configuration frame."""
    payload = bytes((1, 1)) + bytes.fromhex(target) + bytes((district, mode))
    frame = {"id": 0x10000001, "data": payload.hex()}
    with socket.create_connection(("127.0.0.1", bus_port), timeout=1) as connection:
        connection.sendall((json.dumps(frame) + "\n").encode())


def throttle(bus_port: int, target: str, address: int, sequence: int,
             speed: int, direction: int = 1, stop_mode: int = 0) -> None:
    payload = (bytes((1, 1)) + bytes.fromhex(target) + (1).to_bytes(4, "big")
               + sequence.to_bytes(4, "big") + bytes((0,))
               + address.to_bytes(2, "big") + bytes((direction, speed, stop_mode, 0)))
    with socket.create_connection(("127.0.0.1", bus_port), timeout=1) as connection:
        connection.sendall((json.dumps({"id": 0x08000001, "data": payload.hex()}) + "\n").encode())


def packet_bytes(signal: dict) -> bytes:
    bits = signal["bits"]
    assert bits[:14] == "1" * 14 and bits[-1] == "1"
    return bytes(int(bits[start + 1:start + 9], 2)
                 for start in range(14, len(bits) - 1, 9))


class BusObserver:
    def __init__(self, port: int):
        self.connection = socket.create_connection(("127.0.0.1", port), timeout=1)
        self.connection.settimeout(0.5)
        self.pending = b""

    def close(self) -> None:
        self.connection.close()

    def throttle_status(self, network_id: str, address: int,
                        condition: Callable[[dict], bool], timeout: float = 4) -> dict:
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            while b"\n" not in self.pending:
                try:
                    self.pending += self.connection.recv(4096)
                except socket.timeout:
                    break
            if b"\n" not in self.pending:
                continue
            line, self.pending = self.pending.split(b"\n", 1)
            frame = json.loads(line)
            if frame["id"] != 0x13000000 | int(network_id, 16):
                continue
            data = bytes.fromhex(frame["data"])
            if len(data) != 20 or data[:2] != b"\x01\x01" or int.from_bytes(data[11:13], "big") != address:
                continue
            status = {"session": int.from_bytes(data[2:6], "big"),
                      "sequence": int.from_bytes(data[6:10], "big"),
                      "address_kind": data[10], "address": address,
                      "direction": data[13], "speed": data[14],
                      "stop_mode": data[15], "state": data[16]}
            last = status
            if condition(status):
                return status
        raise AssertionError(f"timed out waiting for PSU throttle {address}; last={last}")

    def psu_status(self, network_id: str, condition: Callable[[dict], bool],
                   timeout: float = 4) -> dict:
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            while b"\n" not in self.pending:
                try:
                    self.pending += self.connection.recv(4096)
                except socket.timeout:
                    break
            if b"\n" not in self.pending:
                continue
            line, self.pending = self.pending.split(b"\n", 1)
            frame = json.loads(line)
            if frame["id"] != 0x12000000 | int(network_id, 16):
                continue
            data = bytes.fromhex(frame["data"])
            if len(data) != 12 or data[:2] != b"\x01\x01":
                continue
            status = {
                "state": data[2], "dcc_enabled": bool(data[3] & 1),
                "monitor_valid": bool(data[3] & 2),
                "link_mv": int.from_bytes(data[4:6], "big"),
                "link_ma": int.from_bytes(data[6:8], "big"),
                "uptime_ms": int.from_bytes(data[8:12], "big"),
            }
            last = status
            if condition(status):
                return status
        raise AssertionError(f"timed out waiting for PSU CAN status of {network_id}; last={last}")

    def status(self, network_id: str, district: int, condition: Callable[[dict], bool],
               timeout: float = 4) -> dict:
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            while b"\n" not in self.pending:
                try:
                    self.pending += self.connection.recv(4096)
                except socket.timeout:
                    break
            if b"\n" not in self.pending:
                continue
            line, self.pending = self.pending.split(b"\n", 1)
            frame = json.loads(line)
            if frame["id"] != 0x11000000 | int(network_id, 16):
                continue
            data = bytes.fromhex(frame["data"])
            if len(data) != 16 or data[:2] != b"\x01\x01" or data[2] != district:
                continue
            status = {
                "mode": data[3], "state": data[4],
                "protection_trip": bool(data[5] & 1), "current_valid": bool(data[5] & 2),
                "average_ma": int.from_bytes(data[6:8], "big"),
                "peak_ma": int.from_bytes(data[8:10], "big"),
                "window_ms": int.from_bytes(data[10:14], "big"),
            }
            last = status
            if condition(status):
                return status
        raise AssertionError(f"timed out waiting for CAN status of {network_id}/{district}; last={last}")


def wait_for(label: str, observe: Callable[[], dict], condition: Callable[[dict], bool], timeout: float) -> dict:
    deadline = time.monotonic() + timeout
    last: dict | str = "no response yet"
    while time.monotonic() < deadline:
        try:
            last = observe()
            if condition(last):
                return last
        except (OSError, RuntimeError, json.JSONDecodeError) as error:
            last = str(error)
        time.sleep(0.02)
    raise AssertionError(f"timed out waiting for {label}; last observation: {last}")


def short_recovery(nodes: list[Node], bus_port: int, observer: BusObserver) -> None:
    for node in nodes:
        initial = wait_for(f"{node.name} online with a peer", node.status,
                           lambda status: status["peers"] >= 1, 12)
        assert all(d["state"] == "disabled" and not d["gate"]
                   for d in initial["districts"]), initial
        for district in (0, 1):
            configure(bus_port, initial["network_id"], district, 1)
        wait_for(
            f"{node.name} online with a peer and district 0 running",
            node.status,
            lambda status: status["peers"] >= 1 and status["districts"][0]["state"] == "running",
            12,
        )
        print(f"{node.name}: ready", flush=True)

    for node in nodes:
        before = node.status()["districts"][0]["trips"]
        node.short(0, True)
        tripped = wait_for(
            f"{node.name} to inhibit district 0 after a short",
            node.status,
            lambda status: status["districts"][0]["trips"] > before
            and not status["districts"][0]["enabled"]
            and not status["districts"][0]["gate"],
            5,
        )
        assert tripped["districts"][1]["state"] == "running", tripped
        node.short(0, False)
        measured = observer.status(node.status()["network_id"], 0,
                                   lambda s: s["protection_trip"] and s["peak_ma"] >= 5_000)
        assert measured["current_valid"] and 0 < measured["window_ms"] < 1_000, measured
        print(f"{node.name}: short inhibited locally", flush=True)
        recovered = wait_for(
            f"{node.name} district 0 to resume after clearing the short",
            node.status,
            lambda status: status["districts"][0]["state"] == "running"
            and status["districts"][0]["enabled"]
            and status["districts"][0]["gate"]
            and not status["districts"][0]["shorted"]
            and status["districts"][0]["clear_probes"] >= 10,
            12,
        )
        assert recovered["districts"][0]["current_ma"] == 150, recovered
        assert recovered["districts"][1]["state"] == "running", recovered
        print(f"{node.name}: recovered after clear probes", flush=True)


def district_config(nodes: list[Node], bus_port: int, observer: BusObserver) -> None:
    for node in nodes:
        initial = wait_for(f"{node.name} online", node.status,
                           lambda status: status["peers"] >= 1, 12)
        assert all(d["state"] == "disabled" and not d["gate"]
                   for d in initial["districts"]), initial
        target = initial["network_id"]
        disabled = observer.status(target, 2, lambda s: s["state"] == 0)
        assert disabled["mode"] == 0 and disabled["average_ma"] == 0
        assert disabled["peak_ma"] == 0 and disabled["current_valid"], disabled
        configure(bus_port, "ffffff", 2, 1)
        configure(bus_port, target, 2, 3)  # Invalid commanded state.
        time.sleep(0.1)
        assert initial["districts"] == node.status()["districts"], node.status()
        node.source_ready(False)
        configure(bus_port, target, 2, 1)
        waiting = wait_for(f"{node.name} waiting for source", node.status,
                           lambda s: s["districts"][2]["state"] == "waiting_for_source", 2)
        assert not waiting["districts"][2]["gate"], waiting
        waiting_report = observer.status(target, 2, lambda s: s["state"] == 1)
        assert waiting_report["average_ma"] == 0 and waiting_report["window_ms"] < 1_000, waiting_report
        node.source_ready(True)
        running = wait_for(f"{node.name} district 2 running", node.status,
                           lambda s: s["districts"][2]["state"] == "running", 12)
        assert all(d["state"] == "disabled" and not d["gate"]
                   for index, d in enumerate(running["districts"]) if index != 2), running
        running_report = observer.status(target, 2, lambda s: s["state"] == 4 and s["mode"] == 1)
        assert running_report["average_ma"] > 0 and running_report["peak_ma"] == 150, running_report
        periodic_report = observer.status(target, 2, lambda s: s["state"] == 4 and s["mode"] == 1)
        assert periodic_report["window_ms"] > 0, periodic_report
        configure(bus_port, target, 2, 2)
        programming = wait_for(f"{node.name} district 2 programming", node.status,
                               lambda s: s["districts"][2]["state"] == "running"
                               and s["districts"][2]["mode"] == "programming", 12)
        assert programming["districts"][2]["gate"], programming
        changed_mode = observer.status(target, 2, lambda s: s["state"] == 4 and s["mode"] == 2)
        assert changed_mode["window_ms"] < 1_000, changed_mode
        configure(bus_port, target, 2, 0)
        stopped = wait_for(f"{node.name} district 2 disabled", node.status,
                           lambda s: s["districts"][2]["state"] == "disabled"
                           and not s["districts"][2]["gate"], 2)
        assert stopped["districts"][2]["mode"] == "disabled", stopped
        disabled_report = observer.status(target, 2, lambda s: s["state"] == 0 and s["mode"] == 0)
        assert disabled_report["window_ms"] < 1_000, disabled_report


def psu_startup(psu: Node, nodes: list[Node], bus_port: int,
                signals: SignalObserver, observer: BusObserver) -> None:
    online = wait_for("PSU online", psu.status,
                      lambda s: s["state"] == "online" and s["link_enabled"]
                      and s["dcc_enabled"], 12)
    assert online["packet_us"] == 5796, online
    power = signals.next("power", lambda s: s["enabled"])
    assert power["voltage_mv"] == 15_000, power
    packet = signals.next("dcc_packet", lambda s: True)
    bits = packet["bits"]
    assert len(bits) == 42 and bits[:14] == "1" * 14 and bits[14] == "0", packet
    assert [int(bits[start:start + 8], 2) for start in (15, 24, 33)] == [0xff, 0, 0xff], packet
    assert bits[23] == bits[32] == "0" and bits[41] == "1", packet
    assert packet["half_us"] == [58 if bit == "1" else 100 for bit in bits], packet
    status = observer.psu_status(online["network_id"], lambda s: s["state"] == 2)
    assert status["dcc_enabled"] and status["monitor_valid"] and status["link_mv"] == 15_000, status
    for node in nodes:
        ready = wait_for(f"{node.name} sees DCC", node.status,
                         lambda s: s["source_ready"] and s["dcc_packets"] > 0, 12)
        configure(bus_port, ready["network_id"], 0, 1)
        wait_for(f"{node.name} district running from PSU DCC", node.status,
                 lambda s: s["districts"][0]["state"] == "running", 12)
    psu.power(19_000)
    wait_for("PSU latches unsafe voltage", psu.status,
             lambda s: s["state"] == "fault" and not s["link_enabled"]
             and not s["dcc_enabled"], 4)
    signals.next("power", lambda s: not s["enabled"])
    for node in nodes:
        wait_for(f"{node.name} loses DCC", node.status,
                 lambda s: not s["source_ready"] and not s["link_power"]
                 and s["districts"][0]["state"] == "waiting_for_source"
                 and not s["districts"][0]["gate"], 4)


def psu_throttle(psu: Node, bus_port: int, signals: SignalObserver,
                 observer: BusObserver) -> None:
    online = wait_for("PSU online", psu.status, lambda s: s["state"] == "online", 12)
    target = online["network_id"]
    throttle(bus_port, "ffffff", 3, 1, 20)
    throttle(bus_port, target, 3, 1, 20)
    applied = observer.throttle_status(target, 3, lambda s: s["sequence"] == 1)
    assert applied["speed"] == 20 and applied["state"] == 1, applied
    first = signals.next("dcc_packet", lambda s: len(s["bits"]) == 51
                         and packet_bytes(s)[0] == 3)
    assert packet_bytes(first) == bytes((3, 0x3f, 0x95, 0xa9)), first
    throttle(bus_port, target, 4, 1, 10)
    observer.throttle_status(target, 4, lambda s: s["speed"] == 10)
    second = signals.next("dcc_packet", lambda s: len(s["bits"]) == 51
                          and packet_bytes(s)[0] == 4)
    assert packet_bytes(second) == bytes((4, 0x3f, 0x8b, 0xb0)), second
    assert psu.status()["loco_count"] == 2
    throttle(bus_port, target, 3, 1, 20)  # An exact repeat is acknowledged.
    observer.throttle_status(target, 3, lambda s: s["sequence"] == 1 and s["speed"] == 20)
    throttle(bus_port, target, 3, 0, 0)  # A stale command cannot restore a state.
    throttle(bus_port, target, 3, 2, 0)
    stopped = observer.throttle_status(target, 3, lambda s: s["sequence"] == 2)
    assert stopped["speed"] == 0, stopped
    signals.next("dcc_packet", lambda s: len(s["bits"]) == 51
                 and packet_bytes(s) == bytes((3, 0x3f, 0x80, 0xbc)))
    fresh = BusObserver(bus_port)
    try:
        periodic = fresh.throttle_status(target, 3, lambda s: s["sequence"] == 2, timeout=2)
        assert periodic["speed"] == 0, periodic
    finally:
        fresh.close()


def run_scenario(name: str) -> None:
    ports = [free_port() for _ in range(5)]
    env = os.environ.copy()
    env.update(
        ARC_SIM_BUS_PORT=str(ports[0]),
        ARC_SIM_NODE_A_PORT=str(ports[1]),
        ARC_SIM_NODE_B_PORT=str(ports[2]),
        ARC_SIM_SIGNALS_PORT=str(ports[3]),
        ARC_SIM_PSU_PORT=str(ports[4]),
    )
    project = f"arc-scenario-{os.getpid()}"
    base = ["docker", "compose", "-p", project, "-f", str(COMPOSE)]
    started = False
    try:
        subprocess.run(base + ["up", "-d", "--build", "--wait"], cwd=ROOT, env=env, check=True, timeout=180)
        started = True
        nodes = [Node("rev1", ports[1]), Node("rev2", ports[2])]
        psu = Node("psu", ports[4])
        observer = BusObserver(ports[0])
        signals = SignalObserver(ports[3])
        try:
            if name == "short_recovery":
                short_recovery(nodes, ports[0], observer)
            elif name == "district_config":
                district_config(nodes, ports[0], observer)
            elif name == "psu_startup":
                psu_startup(psu, nodes, ports[0], signals, observer)
            elif name == "psu_throttle":
                psu_throttle(psu, ports[0], signals, observer)
            else:
                raise ValueError(f"unknown scenario: {name}")
        finally:
            observer.close()
            signals.close()
        print(f"PASS {name}", flush=True)
    except Exception:
        if started:
            subprocess.run(base + ["logs", "--no-color", "--tail=80"], cwd=ROOT, env=env, check=False)
        raise
    finally:
        subprocess.run(base + ["down", "--remove-orphans"], cwd=ROOT, env=env, check=False, stdout=subprocess.DEVNULL)


def main() -> None:
    parser = argparse.ArgumentParser(description="Run an ARC firmware integration scenario")
    parser.add_argument("scenario", choices=["short_recovery", "district_config", "psu_startup", "psu_throttle"])
    args = parser.parse_args()
    run_scenario(args.scenario)


if __name__ == "__main__":
    main()
