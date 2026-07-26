#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["websockets"]
# ///
"""A fake arcade-chess board, speaking the real device WebSocket protocol.

This exists so puzzle mode can be built and rehearsed with no hardware on the
desk. It drives the actual `/board` endpoint rather than an in-process shortcut,
which is the point: that is what exercises seq-gap recovery, the
snapshot/incremental mix, and the exact JSON shapes the ESP emits.

Two behaviours are modelled carefully because the server's move matcher depends
on them (see `docs/websocket-api.md` and `firmware-atmega/src/sensors.cpp`):

* **Polarity is random per piece and stable while the piece stays upright.** It
  carries no colour or type information, and a piece put back the wrong way
  round flips it.
* **`uncertain` is only reachable from an occupied state.** It is the
  piece-being-lifted signal and never fires on an empty square. In a
  `board.snapshot` it is indistinguishable from empty, and clears `valid` —
  which is exactly the trap the server has to discriminate with
  `online_node_mask`.

Examples:

    tools/fake-board.py                          # ws://localhost:8080/board
    tools/fake-board.py ws://host:8080/board --token secret
    tools/fake-board.py --setup 8/6k1/8/8/8/8/1R4K1/1r6

Then, at the `board>` prompt:

    move b2b6              a realistic sloppy lift/hover/place
    capture d2d5 victim    lift the victim first, then the attacker
    place e4 / lift e4     one square at a time
    wobble e4              a hand hovering; hold the state machine open
    stick h5 occupied      a sensor that lies until you `unstick` it
    node 2 offline         drop a quadrant
    gap 5                  skip five sequence numbers, forcing a snapshot heal
    fen <FEN>              rebuild the whole board from a position
    snapshot / status / help / quit
"""

import argparse
import asyncio
import contextlib
import json
import random
import sys

import websockets

SQUARES = 64
NODES = 4
FILES = "abcdefgh"
HEARTBEAT_MS = 15_000
# The firmware needs three agreeing scans at ~16 ms each, so a transition lands
# about this long after the physical event. Reproducing the delay keeps the
# server's settle window honest.
TRANSITION_MS = 50


def square_index(name: str) -> int:
    """`e4` -> 28. a1 = 0, h1 = 7, a8 = 56: row-major, the one convention."""
    name = name.strip().lower()
    if len(name) != 2 or name[0] not in FILES or not name[1].isdigit():
        raise ValueError(f"not a square: {name!r}")
    rank = int(name[1]) - 1
    if not 0 <= rank <= 7:
        raise ValueError(f"not a square: {name!r}")
    return rank * 8 + FILES.index(name[0])


def square_name(index: int) -> str:
    return f"{FILES[index % 8]}{index // 8 + 1}"


def node_of(index: int) -> int:
    """Quadrant ownership, per docs/client-api.md."""
    return (index // 8 // 4) * 2 + (index % 8) // 4


def squares_from_fen(fen: str) -> list[int]:
    """Occupied squares of a FEN board field. Piece identity is discarded — the
    board cannot see it either."""
    board = fen.split()[0]
    occupied = []
    rank = 7
    file = 0
    for char in board:
        if char == "/":
            rank -= 1
            file = 0
        elif char.isdigit():
            file += int(char)
        else:
            occupied.append(rank * 8 + file)
            file += 1
    return occupied


class Board:
    """Sensor truth. `state` is what the ADC classifier would report."""

    def __init__(self) -> None:
        self.state = ["empty"] * SQUARES
        # Polarity is a property of how the magnet was glued in, so it is
        # remembered per piece rather than regenerated on each event.
        self.polarity = [random.choice(["positive", "negative"]) for _ in range(SQUARES)]
        self.node_online = [True] * NODES
        self.stuck: dict[int, str] = {}

    def reported(self, index: int) -> str:
        if index in self.stuck:
            return self.stuck[index]
        return self.state[index]

    def snapshot(self) -> dict:
        squares, valid, nodes = [], [], []
        for node in range(NODES):
            nodes.append(
                {
                    "node": node,
                    "online": self.node_online[node],
                    "calibrated": True,
                    "timeouts": 0,
                }
            )
        for index in range(SQUARES):
            state = self.reported(index)
            online = self.node_online[node_of(index)]
            # `uncertain` maps to the same 0 as empty and clears `valid`, which
            # is why the server cannot read `valid` as "offline".
            squares.append(1 if state == "positive" else -1 if state == "negative" else 0)
            valid.append(online and state != "uncertain")
        mask = sum(1 << n for n in range(NODES) if self.node_online[n])
        return {
            "squares": squares,
            "valid": valid,
            "nodes": nodes,
            "online_node_mask": mask,
            "online_node_count": sum(self.node_online),
        }


class FakeBoard:
    def __init__(self, url: str, device_id: str, token: str | None) -> None:
        self.url = url
        self.device_id = device_id
        self.token = token
        self.board = Board()
        self.socket: websockets.ClientConnection | None = None
        self.boot_id = f"{random.randrange(1 << 32):08x}"
        self.seq = 0
        self.at_ms = 0
        self.welcomed = False
        # Sequence numbers to swallow, for rehearsing the gap-heal path.
        self.skip_events = 0

    # ── Wire ──────────────────────────────────────────────────────────────

    def next_envelope(self, etype: str, data: dict) -> dict:
        self.seq = (self.seq + 1) % (1 << 32)
        self.at_ms += TRANSITION_MS
        return {
            "v": 1,
            "type": etype,
            "device_id": self.device_id,
            "boot_id": self.boot_id,
            "seq": self.seq,
            "at_ms": self.at_ms,
            "data": data,
        }

    async def emit(self, etype: str, data: dict) -> None:
        envelope = self.next_envelope(etype, data)
        if self.skip_events > 0 and etype != "board.snapshot":
            # Burn the sequence number without sending it. The server sees a
            # gap and asks for a fresh snapshot, which is the real recovery path.
            self.skip_events -= 1
            print(f"  (dropped seq {envelope['seq']} {etype})")
            return
        assert self.socket is not None
        await self.socket.send(json.dumps(envelope))

    async def send_snapshot(self) -> None:
        await self.emit("board.snapshot", self.board.snapshot())

    async def send_status(self) -> None:
        await self.emit(
            "device.status",
            {
                "rssi": -52,
                "heap": 190_000,
                "uptime_ms": self.at_ms,
                "websocket_reconnects": 0,
                "uart_good": 1000,
                "uart_bad": 0,
                "uart_timeouts": 0,
                "quadrant_mask": sum(
                    1 << n for n in range(NODES) if self.board.node_online[n]
                ),
                "quadrant_count": sum(self.board.node_online),
                "ws_send_failed": 0,
                "events_dropped_offline": 0,
                "snapshot_repairs": 0,
                "raw_stream": False,
                "trace": False,
                "reset_reason": 1,
            },
        )

    async def send_node_status(self, node: int) -> None:
        await self.emit(
            "node.status",
            {
                "node": node,
                "online": self.board.node_online[node],
                "calibrated": True,
                "firmware": "fake-1.0.0",
                "reset_cause": 0,
                "reboots": 0,
                "timeouts": 0,
                "consecutive_timeouts": 0,
                "last_seen_ms": self.at_ms,
            },
        )

    async def sensor_changed(self, index: int, state: str) -> None:
        """One transition, only if the owning quadrant is up. An offline
        quadrant drops its events on the floor, exactly like the real ESP."""
        if not self.board.node_online[node_of(index)]:
            return
        await self.emit(
            "sensor.changed",
            {
                "square": index,
                "state": state,
                "raw": 512 + (90 if state == "positive" else -90 if state == "negative" else 0),
                "baseline": 512,
                "node": node_of(index),
                "local_square": (index // 8 % 4) * 4 + index % 4,
            },
        )

    # ── Physical actions ──────────────────────────────────────────────────

    async def do_lift(self, index: int, flicker: bool = True) -> None:
        """Take a piece off. `uncertain` first, because that is the only way the
        firmware ever reports it — and it is the evidence a capture needs."""
        if self.board.state[index] == "empty":
            return
        if flicker:
            self.board.state[index] = "uncertain"
            await self.sensor_changed(index, "uncertain")
            await asyncio.sleep(TRANSITION_MS / 1000)
        self.board.state[index] = "empty"
        await self.sensor_changed(index, "empty")

    async def do_place(self, index: int, flip: bool = False) -> None:
        if flip:
            self.board.polarity[index] = (
                "negative" if self.board.polarity[index] == "positive" else "positive"
            )
        state = self.board.polarity[index]
        self.board.state[index] = state
        await self.sensor_changed(index, state)

    async def do_move(self, origin: int, target: int, sloppy: bool = True) -> None:
        """A human hand: lift, hover somewhere pointless, then place. The
        polarity travels with the piece."""
        polarity = self.board.polarity[origin]
        await self.do_lift(origin)
        if sloppy and random.random() < 0.4:
            # A hand passing over an empty square produces nothing at all — the
            # classifier only reaches `uncertain` from an occupied state.
            await asyncio.sleep(0.15)
        self.board.polarity[target] = polarity
        await asyncio.sleep(0.1)
        await self.do_place(target)

    async def do_capture(self, origin: int, target: int, order: str) -> None:
        if order.startswith("victim"):
            await self.do_lift(target)
            await asyncio.sleep(0.2)
            await self.do_move(origin, target)
        else:
            polarity = self.board.polarity[origin]
            await self.do_lift(origin)
            await asyncio.sleep(0.2)
            await self.do_lift(target)
            await asyncio.sleep(0.15)
            self.board.polarity[target] = polarity
            await self.do_place(target)

    async def set_position(self, fen: str) -> None:
        occupied = set(squares_from_fen(fen))
        for index in range(SQUARES):
            self.board.state[index] = (
                self.board.polarity[index] if index in occupied else "empty"
            )
        await self.send_snapshot()

    # ── Session ───────────────────────────────────────────────────────────

    async def run(self, setup_fen: str | None) -> None:
        headers = {"Authorization": f"Bearer {self.token}"} if self.token else None
        async with websockets.connect(self.url, additional_headers=headers) as socket:
            self.socket = socket
            await socket.send(
                json.dumps(
                    {
                        "v": 1,
                        "type": "hello",
                        "device_id": self.device_id,
                        "boot_id": self.boot_id,
                        "firmware": "fake-1.0.0",
                        "hardware": "fake-board",
                        "protocols": {"uart": 1, "websocket": 1},
                        "last_server_seq": 0,
                        "capabilities": [
                            "board.snapshot",
                            "sensor.events",
                            "lighting.basic",
                            "diagnostics",
                        ],
                    }
                )
            )
            reader = asyncio.create_task(self.read_loop())
            beat = asyncio.create_task(self.heartbeat_loop())
            # The server replies `welcome` with snapshot_required, but do not
            # block the prompt on it: a server that never answers should still
            # leave a usable console.
            await asyncio.sleep(0.3)
            if setup_fen:
                await self.set_position(setup_fen)
            else:
                await self.send_snapshot()
            await self.send_status()
            for node in range(NODES):
                await self.send_node_status(node)
            try:
                await self.prompt_loop()
            finally:
                reader.cancel()
                beat.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await reader
                with contextlib.suppress(asyncio.CancelledError):
                    await beat

    async def read_loop(self) -> None:
        assert self.socket is not None
        async for raw in self.socket:
            try:
                message = json.loads(raw)
            except json.JSONDecodeError:
                continue
            kind = message.get("type")
            if kind == "welcome":
                self.welcomed = True
                print("\n  <- welcome")
                continue
            if kind != "command":
                continue
            name = message.get("name", "")
            args = message.get("args", {})
            await self.handle_command(message.get("id", ""), name, args)

    async def handle_command(self, cid: str, name: str, args: dict) -> None:
        """Answers every command the server can send. Lighting is acknowledged
        and printed rather than rendered — what matters to the server is the
        terminal `command.result`, and that a rich frame can be *refused*."""
        assert self.socket is not None

        async def result(status: str, reason=None, data=None) -> None:
            payload = {
                "v": 1,
                "type": "command.result",
                "device_id": self.device_id,
                "id": cid,
                "status": status,
                "reason": reason,
            }
            if data is not None:
                payload["data"] = data
            await self.socket.send(json.dumps(payload))

        if name == "board.snapshot.get":
            await result("accepted")
            await self.send_snapshot()
            await result("applied")
            return

        if name in ("lighting.paint", "lighting.bar") and not RICH_LIGHTING:
            # Stock AVR firmware answers an unknown message type with error
            # code 2. That single reply is the server's whole capability
            # discovery, so it has to be reproducible here.
            print(f"  <- {name} refused (node_error code=2)")
            await result("rejected", "node_error", {"node": args.get("node", 0), "code": 2})
            return

        if name.startswith("lighting.") or name == "node.config.set":
            summary = json.dumps(args)
            print(f"  <- {name} {summary[:110]}")
            await result("accepted")
            await result("applied")
            return

        await result("accepted")
        await result("applied")

    async def heartbeat_loop(self) -> None:
        while True:
            await asyncio.sleep(HEARTBEAT_MS / 1000)
            await self.send_status()

    # ── Console ───────────────────────────────────────────────────────────

    async def prompt_loop(self) -> None:
        loop = asyncio.get_running_loop()
        print(__doc__.split("Examples:")[0].strip().splitlines()[0])
        print(f"connected to {self.url} as {self.device_id}; `help` for commands\n")
        while True:
            try:
                line = await loop.run_in_executor(None, lambda: input("board> "))
            except (EOFError, KeyboardInterrupt):
                print()
                return
            line = line.strip()
            if not line:
                continue
            try:
                if await self.dispatch(line):
                    return
            except ValueError as err:
                print(f"  ! {err}")

    async def dispatch(self, line: str) -> bool:
        parts = line.split()
        verb, rest = parts[0].lower(), parts[1:]

        if verb in ("quit", "exit"):
            return True
        if verb == "help":
            print(HELP)
            return False
        if verb == "status":
            self.print_status()
            return False
        if verb == "snapshot":
            await self.send_snapshot()
            print("  -> board.snapshot")
            return False
        if verb == "fen":
            await self.set_position(" ".join(rest))
            print(f"  -> rebuilt from FEN ({len(squares_from_fen(' '.join(rest)))} pieces)")
            return False
        if verb == "place":
            for name in rest:
                await self.do_place(square_index(name))
            return False
        if verb == "flip":
            for name in rest:
                index = square_index(name)
                await self.do_lift(index)
                await self.do_place(index, flip=True)
            return False
        if verb == "lift":
            for name in rest:
                await self.do_lift(square_index(name))
            return False
        if verb == "move":
            if not rest:
                raise ValueError("move e2e4")
            uci = rest[0]
            await self.do_move(square_index(uci[0:2]), square_index(uci[2:4]))
            return False
        if verb == "capture":
            if not rest:
                raise ValueError("capture e4d5 [victim|attacker]")
            uci = rest[0]
            order = rest[1] if len(rest) > 1 else "victim"
            await self.do_capture(square_index(uci[0:2]), square_index(uci[2:4]), order)
            return False
        if verb == "wobble":
            for name in rest:
                index = square_index(name)
                self.board.state[index] = "uncertain"
                await self.sensor_changed(index, "uncertain")
            return False
        if verb == "stick":
            if len(rest) < 2:
                raise ValueError("stick h5 occupied|empty")
            index = square_index(rest[0])
            wanted = rest[1].lower()
            self.board.stuck[index] = (
                self.board.polarity[index] if wanted.startswith("occ") else "empty"
            )
            await self.sensor_changed(index, self.board.stuck[index])
            print(f"  -> {rest[0]} now lies: {self.board.stuck[index]}")
            return False
        if verb == "unstick":
            for name in rest:
                index = square_index(name)
                self.board.stuck.pop(index, None)
                await self.sensor_changed(index, self.board.state[index])
            return False
        if verb == "node":
            if len(rest) < 2:
                raise ValueError("node 2 offline|online")
            node = int(rest[0])
            self.board.node_online[node] = rest[1].lower() == "online"
            await self.send_node_status(node)
            await self.send_snapshot()
            print(f"  -> node {node} {'online' if self.board.node_online[node] else 'offline'}")
            return False
        if verb == "gap":
            self.skip_events = int(rest[0]) if rest else 1
            print(f"  -> dropping the next {self.skip_events} events")
            return False
        if verb == "reboot":
            # A fresh boot_id resets seq to zero and invalidates everything the
            # server thought it knew.
            self.boot_id = f"{random.randrange(1 << 32):08x}"
            self.seq = 0
            print(f"  -> new boot_id {self.boot_id}")
            await self.send_snapshot()
            return False

        raise ValueError(f"unknown command {verb!r}; try `help`")

    def print_status(self) -> None:
        rows = []
        for rank in range(7, -1, -1):
            cells = []
            for file in range(8):
                index = rank * 8 + file
                state = self.board.reported(index)
                mark = {"positive": "+", "negative": "-", "uncertain": "?", "empty": "."}[state]
                if not self.board.node_online[node_of(index)]:
                    mark = "x"
                cells.append(mark)
            rows.append(f"  {rank + 1} " + " ".join(cells))
        print("\n".join(rows))
        print("    " + " ".join(FILES))
        print(
            f"  boot={self.boot_id} seq={self.seq} "
            f"nodes={''.join('1' if n else '0' for n in self.board.node_online)} "
            f"stuck={[square_name(i) for i in self.board.stuck]}"
        )


HELP = """\
  move <uci>              lift, hover, place — the sloppy human version
  capture <uci> [victim|attacker]   capture, choosing which piece leaves first
  place <sq>...           put a piece down
  lift <sq>...            take a piece off (uncertain, then empty)
  flip <sq>...            lift and replace the wrong way round (polarity flips)
  wobble <sq>...          leave a square mid-transition, as a hovering hand does
  stick <sq> occupied|empty     a sensor that lies until `unstick`
  unstick <sq>...         tell the truth again
  node <n> online|offline drop or restore a quadrant
  gap <n>                 swallow n sequence numbers, forcing a snapshot heal
  reboot                  new boot_id, seq back to zero
  fen <FEN>               rebuild the whole board
  snapshot | status | help | quit\
"""

RICH_LIGHTING = False


def main() -> int:
    global RICH_LIGHTING
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "url", nargs="?", default="ws://localhost:8080/board", help="device endpoint"
    )
    parser.add_argument("--device-id", default="arcade-chess-fake")
    parser.add_argument("--token", default=None, help="DEVICE_TOKEN, if the server wants one")
    parser.add_argument("--setup", default=None, help="FEN to build the board from at startup")
    parser.add_argument(
        "--rich-lighting",
        action="store_true",
        help="accept lighting.paint / lighting.bar instead of refusing with node_error 2, "
        "i.e. pretend the quadrants run the new AVR firmware",
    )
    parser.add_argument("--seed", type=int, default=None, help="fix the polarity draw")
    args = parser.parse_args()

    if args.seed is not None:
        random.seed(args.seed)
    RICH_LIGHTING = args.rich_lighting

    board = FakeBoard(args.url, args.device_id, args.token)
    try:
        asyncio.run(board.run(args.setup))
    except (KeyboardInterrupt, asyncio.CancelledError):
        pass
    except OSError as err:
        print(f"could not connect to {args.url}: {err}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
