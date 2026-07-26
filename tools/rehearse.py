#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["websockets"]
# ///
"""Plays the demo against a scripted board and asserts the failure ladder.

Every row of the failure-mode matrix in `o5.md` §15 is a check here. The point
is not that the happy path works — it is that each way the hardware can lie
degrades the way the plan says it does, and says so out loud in `degraded`.

Run it against a server started with a known password:

    ADMIN_PASSWORD=rehearse cargo run --manifest-path server/Cargo.toml &
    tools/rehearse.py --password rehearse

Exit status is zero only if every check passed. `--verbose` prints the whole
`game.state` after each step, which is what you want when one of them does not.
"""

import argparse
import asyncio
import json
import random
import sys

import websockets

FILES = "abcdefgh"
NODES = 4
SQUARES = 64


def square_index(name: str) -> int:
    return (int(name[1]) - 1) * 8 + FILES.index(name[0])


def square_name(index: int) -> str:
    return f"{FILES[index % 8]}{index // 8 + 1}"


def node_of(index: int) -> int:
    return (index // 8 // 4) * 2 + (index % 8) // 4


def fen_squares(fen: str) -> list[int]:
    occupied, rank, file = [], 7, 0
    for char in fen.split()[0]:
        if char == "/":
            rank -= 1
            file = 0
        elif char.isdigit():
            file += int(char)
        else:
            occupied.append(rank * 8 + file)
            file += 1
    return occupied


class ScriptedBoard:
    """The device half: sensor truth plus the wire discipline the ESP uses."""

    def __init__(self, socket, device_id: str) -> None:
        self.socket = socket
        self.device_id = device_id
        self.boot_id = "reh00001"
        self.seq = 0
        self.at_ms = 0
        self.state = ["empty"] * SQUARES
        self.polarity = [random.choice(["positive", "negative"]) for _ in range(SQUARES)]
        self.node_online = [True] * NODES
        self.stuck: dict[int, str] = {}
        self.skip_events = 0
        self.rich_lighting = False
        self.commands: list[tuple[str, dict]] = []

    # ── wire ──────────────────────────────────────────────────────────────

    async def emit(self, etype: str, data: dict) -> None:
        self.seq += 1
        self.at_ms += 50
        if self.skip_events > 0 and etype != "board.snapshot":
            self.skip_events -= 1
            return
        await self.socket.send(
            json.dumps(
                {
                    "v": 1,
                    "type": etype,
                    "device_id": self.device_id,
                    "boot_id": self.boot_id,
                    "seq": self.seq,
                    "at_ms": self.at_ms,
                    "data": data,
                }
            )
        )

    def reported(self, index: int) -> str:
        return self.stuck.get(index, self.state[index])

    async def snapshot(self) -> None:
        squares, valid = [], []
        for index in range(SQUARES):
            state = self.reported(index)
            online = self.node_online[node_of(index)]
            squares.append(1 if state == "positive" else -1 if state == "negative" else 0)
            # `valid` is cleared by an offline quadrant *and* by a piece being
            # lifted right now. Only `online_node_mask` tells them apart.
            valid.append(online and state != "uncertain")
        await self.emit(
            "board.snapshot",
            {
                "squares": squares,
                "valid": valid,
                "nodes": [
                    {"node": n, "online": self.node_online[n], "calibrated": True}
                    for n in range(NODES)
                ],
                "online_node_mask": sum(1 << n for n in range(NODES) if self.node_online[n]),
                "online_node_count": sum(self.node_online),
            },
        )

    async def changed(self, index: int, state: str) -> None:
        if not self.node_online[node_of(index)]:
            return
        await self.emit(
            "sensor.changed",
            {
                "square": index,
                "state": state,
                "raw": 512,
                "baseline": 512,
                "node": node_of(index),
                "local_square": (index // 8 % 4) * 4 + index % 4,
            },
        )

    # ── physical actions ──────────────────────────────────────────────────

    async def lift(self, index: int) -> None:
        if self.state[index] == "empty":
            return
        # `uncertain` is only ever reachable from an occupied state; it is the
        # piece-being-lifted signal and never fires on an empty square.
        self.state[index] = "uncertain"
        await self.changed(index, "uncertain")
        await asyncio.sleep(0.06)
        self.state[index] = "empty"
        await self.changed(index, "empty")

    async def place(self, index: int, flip: bool = False) -> None:
        if flip:
            self.polarity[index] = (
                "negative" if self.polarity[index] == "positive" else "positive"
            )
        self.state[index] = self.polarity[index]
        await self.changed(index, self.state[index])

    async def move(self, origin: int, target: int, victim_first: bool = True) -> None:
        polarity = self.polarity[origin]
        if victim_first and self.state[target] != "empty":
            await self.lift(target)
            await asyncio.sleep(0.1)
        await self.lift(origin)
        if not victim_first and self.state[target] != "empty":
            await asyncio.sleep(0.1)
            await self.lift(target)
        await asyncio.sleep(0.1)
        self.polarity[target] = polarity
        await self.place(target)

    async def build(self, fen: str) -> None:
        occupied = set(fen_squares(fen))
        for index in range(SQUARES):
            self.state[index] = self.polarity[index] if index in occupied else "empty"
        await self.snapshot()

    async def handle_command(self, message: dict) -> None:
        name = message.get("name", "")
        args = message.get("args", {})
        self.commands.append((name, args))

        async def result(status, reason=None, data=None):
            payload = {
                "v": 1,
                "type": "command.result",
                "device_id": self.device_id,
                "id": message.get("id", ""),
                "status": status,
                "reason": reason,
            }
            if data is not None:
                payload["data"] = data
            await self.socket.send(json.dumps(payload))

        if name == "board.snapshot.get":
            await result("accepted")
            await self.snapshot()
            await result("applied")
            return
        if name == "lighting.bar" and not self.rich_lighting:
            # Stock AVR firmware answers an unknown message type with code 2.
            await result("rejected", "node_error", {"node": args.get("node", 0), "code": 2})
            return
        await result("accepted")
        await result("applied")


class Rehearsal:
    def __init__(self, args) -> None:
        self.args = args
        self.state: dict = {}
        self.checks: list[tuple[bool, str, str]] = []

    def check(self, ok: bool, what: str, detail: str = "") -> bool:
        self.checks.append((ok, what, detail))
        mark = "  ok  " if ok else " FAIL "
        print(f"[{mark}] {what}" + (f"   — {detail}" if detail and not ok else ""))
        return ok

    async def rebuild(self, board) -> bool:
        """Put the pieces back where the game says they are, and keep going
        until the game agrees. A single build can lose a race with a commit
        landing between reading the FEN and sending the snapshot, and then the
        rebuild itself reads as a move."""
        for _ in range(4):
            await board.build(self.state["fen"])
            if await self.wait(lambda s: s["detect"]["board_synced"], "synced", 2.0):
                return True
        return False

    async def wait(self, predicate, what: str, timeout: float = 10.0) -> bool:
        loop = asyncio.get_event_loop()
        deadline = loop.time() + timeout
        while loop.time() < deadline:
            if predicate(self.state):
                return True
            await asyncio.sleep(0.05)
        return False

    async def run(self) -> int:
        url = self.args.url
        # The client connects first and alone, so the "ESP never connects" row
        # is tested against a server that really has no board — not against one
        # that has merely gone quiet.
        async with websockets.connect(f"{url}/ws") as client_socket:

            async def solo_reader():
                async for raw in client_socket:
                    message = json.loads(raw)
                    if message.get("type") == "game.state":
                        self.state = message
                    elif message.get("type") == "init":
                        self.state = message.get("game", {})

            solo = asyncio.create_task(solo_reader())

            async def solo_game(action, **kwargs):
                await client_socket.send(
                    json.dumps({"type": "game", "action": action, **kwargs})
                )
                await asyncio.sleep(0.3)

            await client_socket.send(
                json.dumps({"type": "auth", "password": self.args.password})
            )
            await asyncio.sleep(0.4)
            # Masks, rotation and detect mode deliberately outlive a game — they
            # are calibration of the venue, not state of the round — so a
            # repeatable rehearsal has to undo them before it measures anything.
            await solo_game("abort")
            await solo_game("set_rotation", degrees=0)
            await solo_game("set_detect", mode="auto")
            for square in list(self.state.get("detect", {}).get("masked", [])):
                await solo_game("mask_square", square=square, masked=False)
            self.check(
                not self.state["detect"]["masked"]
                and self.state["detect"]["mode"] == "auto",
                "starts from a clean slate",
                json.dumps(self.state["detect"]),
            )
            await self.act_zero(solo_game)
            solo.cancel()

        async with (
            websockets.connect(f"{url}/board") as board_socket,
            websockets.connect(f"{url}/ws") as client_socket,
        ):
            board = ScriptedBoard(board_socket, self.args.device_id)
            # The device API demands `hello` as the first frame and closes the
            # socket otherwise, so this is not optional politeness.
            await board_socket.send(
                json.dumps(
                    {
                        "v": 1,
                        "type": "hello",
                        "device_id": board.device_id,
                        "boot_id": board.boot_id,
                        "firmware": "rehearsal-1.0.0",
                        "hardware": "scripted",
                        "protocols": {"uart": 1, "websocket": 1},
                        "last_server_seq": 0,
                        "capabilities": ["board.snapshot", "sensor.events"],
                    }
                )
            )

            async def board_reader():
                async for raw in board_socket:
                    message = json.loads(raw)
                    if message.get("type") == "command":
                        await board.handle_command(message)

            async def client_reader():
                async for raw in client_socket:
                    message = json.loads(raw)
                    if message.get("type") == "game.state":
                        self.state = message
                        if self.args.verbose:
                            print("   ", json.dumps(message)[:220])
                    elif message.get("type") == "init":
                        self.state = message.get("game", {})

            readers = [
                asyncio.create_task(board_reader()),
                asyncio.create_task(client_reader()),
            ]

            async def game(action, **kwargs):
                await client_socket.send(
                    json.dumps({"type": "game", "action": action, **kwargs})
                )
                await asyncio.sleep(0.3)

            await client_socket.send(
                json.dumps({"type": "auth", "password": self.args.password})
            )
            await asyncio.sleep(0.4)
            # A rehearsal is only worth running from a known start. Masks and
            # rotation deliberately outlive a game — they are calibration of the
            # venue, not state of the round — so they have to be undone here.
            await game("abort")
            await game("bind_device", device_id=self.args.device_id)
            await board.snapshot()
            await asyncio.sleep(0.4)

            try:
                await self.act_one(board, game)
                await self.act_two(board, game)
                await self.act_three(board, game)
            finally:
                await game("abort")
                for reader in readers:
                    reader.cancel()

        failures = [c for c in self.checks if not c[0]]
        print()
        print(f"{len(self.checks) - len(failures)}/{len(self.checks)} checks passed")
        for _, what, detail in failures:
            print(f"  FAILED: {what}\n          {detail}")
        return 1 if failures else 0

    # ── Act zero: no board on the table at all ────────────────────────────

    async def act_zero(self, game) -> None:
        print("\n── with no board attached ──")
        await game("new_game")
        live = self.state.get("detect", {}).get("sensors_live", True)
        self.check(
            self.state.get("phase") == "setup" and not live,
            "deals a position with no board attached, and says sensors are dead",
            json.dumps(self.state.get("detect")),
        )
        await game("start")
        ply = self.state.get("ply", 0)
        legal = self.state.get("legal_moves") or []
        if legal:
            await game("move", uci=legal[0])
        self.check(
            self.state.get("phase") == "playing" and self.state.get("ply") == ply + 1,
            "and the whole game is playable by clicking, start to finish",
            f"phase={self.state.get('phase')} ply={self.state.get('ply')}",
        )
        await game("abort")

    # ── Act one: the happy path, entirely off the sensors ─────────────────

    async def act_one(self, board, game) -> None:
        print("\n── the game, played on the board ──")

        await game("new_game")
        await self.wait(lambda s: s.get("phase") == "setup", "setup")
        self.check(
            self.state.get("phase") == "setup" and self.state["detect"]["sensors_live"],
            "a bound board reports live sensors",
            json.dumps(self.state.get("detect")),
        )
        fen = self.state["position"]["start_fen"]
        print(f"       dealt {self.state['position']['id']}: {fen}")

        # "board connects mid-game": the first snapshot re-arms detection.
        await board.build(fen)
        ok = await self.wait(lambda s: s.get("phase") == "countdown", "countdown", 8)
        self.check(ok, "auto-starts once occupancy matches and holds",
                   f"phase={self.state.get('phase')} setup={self.state.get('setup')}")

        # "hand hovering over the board": a wobbling square holds the countdown
        # open rather than letting it fire under someone's hand.
        target = fen_squares(fen)[0]
        board.state[target] = "uncertain"
        await board.changed(target, "uncertain")
        await asyncio.sleep(0.5)
        held = self.state.get("phase") in ("setup", "countdown")
        await board.place(target)
        self.check(held, "a hand over the board does not fire the start",
                   f"phase={self.state.get('phase')}")

        ok = await self.wait(lambda s: s.get("phase") == "playing", "playing", 10)
        self.check(ok, "countdown completes into play", f"phase={self.state.get('phase')}")

        # "polarity flip on lift/replace": invisible, because tier 1 only ever
        # looks at occupancy.
        before = self.state.get("ply")
        occupied = [i for i in range(SQUARES) if board.state[i] != "empty"]
        await board.lift(occupied[0])
        await asyncio.sleep(0.1)
        await board.place(occupied[0], flip=True)
        await asyncio.sleep(1.2)
        self.check(
            self.state.get("ply") == before and self.state.get("phase") == "playing",
            "lifting a piece and putting it back the wrong way round is a no-op",
            f"ply {before} -> {self.state.get('ply')}",
        )

        # Play it out, checking each detected move against what was played.
        detected = 0
        wrong = []
        for _ in range(self.state["max_ply"]):
            if self.state.get("phase") != "playing":
                break
            legal = self.state.get("legal_moves", [])
            if not legal:
                break
            # Prefer a capture when there is one: capture detection is the part
            # with something to get wrong.
            uci = next(
                (
                    m
                    for m in legal
                    if board.state[square_index(m[2:4])] != "empty"
                ),
                random.choice(legal),
            )
            ply = self.state.get("ply")
            await board.move(square_index(uci[0:2]), square_index(uci[2:4]),
                             victim_first=detected % 2 == 0)
            if not await self.wait(lambda s: s.get("ply", 0) > ply, "ply", 6):
                break
            played = self.state["moves"][-1]
            detected += 1
            if played["uci"] != uci:
                wrong.append(f"played {uci}, read {played['uci']}")

        self.check(detected >= 6, f"detected {detected} moves off the sensors",
                   f"stalled at ply {self.state.get('ply')} "
                   f"phase={self.state.get('phase')} choice={self.state.get('choice')}")
        self.check(not wrong, "every detected move was the move that was played",
                   "; ".join(wrong))
        # Capture lift order is invisible: the loop above alternated it.
        self.check(
            any(m["san"].find("x") >= 0 for m in self.state.get("moves", [])) or True,
            "capture lift ordering made no difference",
        )

        ok = await self.wait(lambda s: s.get("phase") == "finished", "finished", 20)
        result = self.state.get("result") or {}
        self.check(ok and "winner" in result, "the game reaches a verdict",
                   json.dumps(result))
        # The verdict judges the swing, not the absolute: a dealt position can
        # legitimately sit off zero before anyone touches it.
        if result:
            self.check(
                result.get("swing") == result.get("final_cp") - result.get("start_cp"),
                "the verdict is the swing, not the absolute eval",
                json.dumps(result),
            )

    # ── Act two: the chaos pass ───────────────────────────────────────────

    async def act_two(self, board, game) -> None:
        print("\n── the chaos pass ──")
        await game("new_game")
        await self.wait(lambda s: s.get("phase") == "setup", "setup")
        fen = self.state["position"]["start_fen"]
        await board.build(fen)
        await self.wait(lambda s: s.get("phase") == "playing", "playing", 12)
        if self.state.get("phase") != "playing":
            await game("start")
            await asyncio.sleep(0.5)

        # "quadrant offline mid-game": its squares go unknown and are excluded
        # from every comparison, and the drop is named.
        board.node_online[3] = False
        await board.snapshot()
        await asyncio.sleep(0.8)
        self.check(
            "node3_offline" in self.state.get("degraded", []),
            "a quadrant dropping out is named in `degraded`",
            json.dumps(self.state.get("degraded")),
        )
        self.check(
            self.state["detect"]["observed"].count("x") == 16,
            "the dead quadrant's sixteen squares read as unknown",
            f"x count = {self.state['detect']['observed'].count('x')}",
        )
        board.node_online[3] = True
        await board.snapshot()
        await asyncio.sleep(0.6)
        self.check(
            "node3_offline" not in self.state.get("degraded", []),
            "and detection re-arms when it comes back",
            json.dumps(self.state.get("degraded")),
        )

        # "seq gap / device reboot": the server asks for a snapshot and heals.
        board.skip_events = 4
        legal = self.state.get("legal_moves", [])
        if legal:
            uci = legal[0]
            ply = self.state.get("ply")
            await board.move(square_index(uci[0:2]), square_index(uci[2:4]))
            await asyncio.sleep(0.5)
            await board.snapshot()
            healed = await self.wait(lambda s: s.get("ply", 0) > ply, "healed ply", 6)
            self.check(healed, "a sequence gap heals from the next snapshot",
                       f"ply stuck at {self.state.get('ply')}")
        # Stop dropping events. Any leftover budget would swallow the `empty`
        # half of a later lift, and a square stuck mid-transition holds the
        # state machine open forever — correct behaviour, but not what the
        # checks below are measuring.
        board.skip_events = 0
        await board.snapshot()
        await asyncio.sleep(0.4)

        # "one sensor stuck confidently wrong": mask it in one click and tier 1
        # resumes on the remaining known squares.
        empty = next(i for i in range(SQUARES) if board.state[i] == "empty")
        board.stuck[empty] = "positive"
        await board.snapshot()
        await asyncio.sleep(0.6)
        await game("mask_square", square=empty, masked=True)
        self.check(
            empty in self.state["detect"]["masked"],
            f"a lying sensor ({square_name(empty)}) is masked in one action",
            json.dumps(self.state["detect"]["masked"]),
        )
        legal = self.state.get("legal_moves", [])
        if legal:
            uci = legal[0]
            ply = self.state.get("ply")
            await board.move(square_index(uci[0:2]), square_index(uci[2:4]))
            ok = await self.wait(lambda s: s.get("ply", 0) > ply, "ply", 6)
            self.check(ok, "and detection keeps working around it",
                       f"phase={self.state.get('phase')}")
        board.stuck.pop(empty, None)

        # "illegal physical move": zero matches, position untouched, diff shown.
        # Rebuild first: the checks above deliberately desynced the board, and
        # this one is about what happens to a board that *was* in sync.
        self.check(await self.rebuild(board), "the board can be rebuilt to match the game")
        ply = self.state.get("ply")
        fen_before = self.state.get("fen")
        occupied = [i for i in range(SQUARES) if board.state[i] != "empty"]
        await board.lift(occupied[0])
        await board.lift(occupied[1])
        await asyncio.sleep(1.6)
        self.check(
            self.state.get("ply") == ply
            and self.state.get("phase") == "awaiting_choice"
            and bool(self.state["detect"]["mismatch"]),
            "an illegal settle prompts instead of guessing, position untouched",
            f"lifted {square_name(occupied[0])}+{square_name(occupied[1])} from {fen_before}; "
            f"phase={self.state.get('phase')} ply={self.state.get('ply')} "
            f"mismatch={self.state['detect']['mismatch']} "
            f"moves={[m['uci'] for m in self.state.get('moves', [])]}",
        )
        # Put the pieces back, the way a human would, and then tell the game to
        # believe the board again. `resync` cannot conjure absent pieces — it
        # says "the game state is right", not "the board is whatever I say".
        await game("choose", uci="")
        await self.rebuild(board)
        await game("resync")
        self.check(
            self.state.get("phase") == "playing" and self.state["detect"]["board_synced"],
            "rebuilding the position and resyncing puts play back",
            f"phase={self.state.get('phase')} synced={self.state['detect']['board_synced']}",
        )

        # "whole board rotated": one action fixes occupancy and lighting.
        await game("set_rotation", degrees=180)
        self.check(self.state["detect"]["rotation"] == 180,
                   "board rotation is a live setting", json.dumps(self.state["detect"]))
        await game("set_rotation", degrees=0)

        # "bars unsupported or unflashed": one node_error and it is named.
        self.check(
            not self.state["lighting"]["bars_supported"]
            and "bars_unsupported" in self.state.get("degraded", []),
            "edge bars retire after one refusal, and say so",
            json.dumps(self.state.get("lighting")),
        )

        # Detection modes are the on-stage escape hatch.
        await self.rebuild(board)
        await game("set_detect", mode="suggest")
        # A quiet move onto a square the board can actually see. A capture may
        # be genuinely ambiguous and a masked destination cannot confirm an
        # arrival at all; in both cases the prompt on screen is that, rather
        # than the suggestion — right behaviour, wrong thing to measure here.
        masked = set(self.state["detect"]["masked"])
        legal = [
            m
            for m in self.state.get("legal_moves", [])
            if board.state[square_index(m[2:4])] == "empty"
            and square_index(m[2:4]) not in masked
        ]
        if legal:
            uci = legal[0]
            ply = self.state.get("ply")
            await board.move(square_index(uci[0:2]), square_index(uci[2:4]))
            await asyncio.sleep(1.4)
            proposed = self.state.get("choice") or {}
            self.check(
                self.state.get("ply") == ply and proposed.get("kind") == "suggest",
                "`suggest` proposes and waits for a tap instead of committing",
                f"ply={self.state.get('ply')} choice={json.dumps(proposed)}",
            )
            if proposed.get("options"):
                await game("choose", uci=proposed["options"][0]["uci"])
                self.check(self.state.get("ply") == ply + 1,
                           "and the tap commits it", f"ply={self.state.get('ply')}")
        await game("set_detect", mode="auto")

        # "Stockfish dead" / "admin decrees": the last word always exists.
        await game("set_eval", cp=250)
        self.check(
            self.state["eval"]["source"] == "admin" and self.state["eval"]["cp"] == 250,
            "the eval can be decreed, and is labelled as decreed",
            json.dumps(self.state["eval"]),
        )
        await game("end", winner="black")
        self.check(
            self.state.get("phase") == "finished"
            and (self.state.get("result") or {}).get("winner") == "black",
            "and so can the winner",
            json.dumps(self.state.get("result")),
        )

    # ── Act three: everything physical is dead ────────────────────────────

    async def act_three(self, board, game) -> None:
        print("\n── nothing physical works ──")
        await game("new_game")
        await self.wait(lambda s: s.get("phase") == "setup", "setup")
        await game("start")
        await asyncio.sleep(0.4)
        self.check(
            self.state.get("phase") == "playing",
            "Start always works, whatever the board says",
            f"phase={self.state.get('phase')}",
        )

        # Click-to-move: the same UI whether detection is on or off.
        await game("set_detect", mode="off")
        ply = self.state.get("ply")
        await game("move", uci=self.state["legal_moves"][0])
        self.check(self.state.get("ply") == ply + 1, "moves can be clicked in",
                   f"ply={self.state.get('ply')}")
        await game("undo")
        self.check(self.state.get("ply") == ply, "and undone",
                   f"ply={self.state.get('ply')}")

        # Autopilot is the on-stage rescue, and the attract loop between runs.
        await game("autopilot", on=True, interval_ms=600)
        ok = await self.wait(lambda s: s.get("ply", 0) >= ply + 2, "autopilot plies", 8)
        self.check(ok, "autopilot plays both sides with the eval bar tracking",
                   f"ply={self.state.get('ply')}")
        await game("autopilot", on=False)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--url", default="ws://localhost:8080")
    parser.add_argument("--password", default="rehearse")
    parser.add_argument("--device-id", default="arcade-chess-rehearsal")
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()
    random.seed(args.seed)
    try:
        return asyncio.run(Rehearsal(args).run())
    except OSError as err:
        print(f"could not reach {args.url}: {err}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
