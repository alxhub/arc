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


def short_recovery(nodes: list[Node]) -> None:
    for node in nodes:
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
        print(f"{node.name}: short inhibited locally", flush=True)
        node.short(0, False)
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


def run_scenario(name: str) -> None:
    ports = [free_port() for _ in range(3)]
    env = os.environ.copy()
    env.update(
        ARC_SIM_BUS_PORT=str(ports[0]),
        ARC_SIM_NODE_A_PORT=str(ports[1]),
        ARC_SIM_NODE_B_PORT=str(ports[2]),
    )
    project = f"arc-scenario-{os.getpid()}"
    base = ["docker", "compose", "-p", project, "-f", str(COMPOSE)]
    started = False
    try:
        subprocess.run(base + ["up", "-d", "--build", "--wait"], cwd=ROOT, env=env, check=True, timeout=180)
        started = True
        nodes = [Node("rev1", ports[1]), Node("rev2", ports[2])]
        if name == "short_recovery":
            short_recovery(nodes)
        else:
            raise ValueError(f"unknown scenario: {name}")
        print(f"PASS {name}", flush=True)
    except Exception:
        if started:
            subprocess.run(base + ["logs", "--no-color", "--tail=80"], cwd=ROOT, env=env, check=False)
        raise
    finally:
        subprocess.run(base + ["down", "--remove-orphans"], cwd=ROOT, env=env, check=False, stdout=subprocess.DEVNULL)


def main() -> None:
    parser = argparse.ArgumentParser(description="Run an ARC firmware integration scenario")
    parser.add_argument("scenario", choices=["short_recovery"])
    args = parser.parse_args()
    run_scenario(args.scenario)


if __name__ == "__main__":
    main()
