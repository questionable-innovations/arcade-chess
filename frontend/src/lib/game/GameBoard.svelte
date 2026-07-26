<script lang="ts">
	// The chess-oriented board. `$lib/Board.svelte` stays sensor-oriented and
	// untouched — the bring-up dashboard is the recovery tool when the
	// electronics act up mid-demo, and it must not regress.
	//
	// Two things this has to be, in order: readable across a room from a
	// projector, and readable at a glance by the operator. That rules out the
	// chrome's near-black square palette (a chessboard needs its own contrast)
	// and it rules out Unicode piece glyphs — the solid black forms disappear on
	// a dark square, and the outline white forms turn to mush at distance.
	//
	// Pieces are absolutely positioned and carry stable ids across FEN changes,
	// so a detected move *glides*. On a board where the moves arrive from
	// sensors rather than from a click, that animation is the thing that tells
	// the room the board was read at all.
	//
	// Manual move entry is not a hidden fallback: this is the same click-to-move
	// UI whether detection is on or off. Detection just does the clicking.

	import { untrack } from 'svelte';
	import {
		boardFromFen,
		pickMove,
		squareName,
		targetsFrom,
		uciSquares,
		type GameState
	} from './types';

	interface Props {
		game: GameState;
		/** Admin-only: clicking a square commits a move. */
		interactive?: boolean;
		/** Arm-then-click square masking from the admin rail. */
		maskArmed?: boolean;
		onMove?: (uci: string) => void;
		onMaskSquare?: (square: number) => void;
	}

	let { game, interactive = false, maskArmed = false, onMove, onMaskSquare }: Props = $props();

	const FILES = 'abcdefgh';
	const CELLS = Array.from({ length: 64 }, (_, square) => ({
		square,
		file: square % 8,
		rank: Math.floor(square / 8),
		dark: (Math.floor(square / 8) + (square % 8)) % 2 === 0
	}));

	// Rank 8 at the top, so white sits at the bottom the way the players do.
	const xOf = (file: number) => file;
	const yOf = (rank: number) => 7 - rank;

	let selected = $state<number | null>(null);

	// During setup the board shows the position to *build*; once play starts it
	// shows the position in play. They agree until the first move, but saying so
	// explicitly is what keeps the countdown honest.
	const building = $derived(game.phase === 'setup' || game.phase === 'countdown');
	const shownFen = $derived(building ? game.start_fen : game.fen);

	const targets = $derived(selected == null ? [] : targetsFrom(game.legal_moves, selected));
	const movable = $derived(
		new Set(game.legal_moves.map((uci) => uciSquares(uci)?.[0]).filter((v) => v != null))
	);
	const lastMove = $derived.by(() => {
		const last = game.moves.at(-1);
		return building ? null : last ? uciSquares(last.uci) : null;
	});
	const missing = $derived(new Set(building ? (game.setup?.missing ?? []) : []));
	const extra = $derived(new Set(building ? (game.setup?.extra ?? []) : []));
	const unknown = $derived(new Set(building ? (game.setup?.unknown ?? []) : []));
	const mismatch = $derived(new Set(game.detect.mismatch));
	const masked = $derived(new Set(game.detect.masked));
	const candidates = $derived(
		new Set(
			(game.choice?.options ?? [])
				.map((option) => uciSquares(option.uci)?.[1])
				.filter((square) => square != null)
		)
	);

	// ── Piece tracking ──────────────────────────────────────────────────────
	// Match the pieces already on screen to the new FEN — same square first,
	// then nearest of the same kind, which covers moves, captures, castling and
	// en passant without any of them being special-cased. Leftovers fade out.

	interface Placed {
		id: number;
		letter: string;
		square: number;
		fresh: boolean;
	}
	let pieces = $state<Placed[]>([]);
	let ghosts = $state<Placed[]>([]);
	let nextId = 1;

	function distance(a: number, b: number): number {
		const df = (a % 8) - (b % 8);
		const dr = Math.floor(a / 8) - Math.floor(b / 8);
		return df * df + dr * dr;
	}

	function sync(fen: string) {
		// Plain arrays rather than Map/Set: this is function-local scratch, and
		// 64 slots indexed by square is both simpler and what the rest of the
		// codebase means by "a board".
		const wanted: (string | null)[] = boardFromFen(fen).map((piece) => piece?.letter ?? null);

		const keep: Placed[] = [];
		const homeless: Placed[] = [];
		for (const piece of pieces) {
			if (wanted[piece.square] === piece.letter) {
				keep.push({ ...piece, fresh: false });
				wanted[piece.square] = null;
			} else {
				homeless.push(piece);
			}
		}

		// Nearest same-kind wins, which covers moves, captures, castling and en
		// passant without any of them being a special case.
		const pairs: { piece: Placed; square: number; d: number }[] = [];
		for (const piece of homeless) {
			wanted.forEach((letter, square) => {
				if (letter === piece.letter) {
					pairs.push({ piece, square, d: distance(piece.square, square) });
				}
			});
		}
		pairs.sort((a, b) => a.d - b.d);
		const claimed: number[] = [];
		for (const { piece, square } of pairs) {
			if (claimed.includes(piece.id) || wanted[square] === null) continue;
			claimed.push(piece.id);
			wanted[square] = null;
			keep.push({ ...piece, square, fresh: false });
		}

		for (const piece of homeless) {
			if (claimed.includes(piece.id)) continue;
			const ghost = { ...piece };
			ghosts = [...ghosts, ghost];
			setTimeout(() => (ghosts = ghosts.filter((g) => g.id !== ghost.id)), 320);
		}
		wanted.forEach((letter, square) => {
			if (letter) keep.push({ id: nextId++, letter, square, fresh: true });
		});
		pieces = keep;
	}

	$effect(() => {
		const fen = shownFen;
		untrack(() => sync(fen));
	});

	const occupied = $derived(new Set(pieces.map((p) => p.square)));
	const source = (letter: string) =>
		`/pieces/${letter === letter.toUpperCase() ? 'w' : 'b'}${letter.toUpperCase()}.svg`;

	function onSquare(square: number) {
		if (maskArmed) {
			onMaskSquare?.(square);
			return;
		}
		if (!interactive) return;
		if (selected != null) {
			const uci = pickMove(game.legal_moves, selected, square);
			if (uci) {
				onMove?.(uci);
				selected = null;
				return;
			}
		}
		selected = movable.has(square) && selected !== square ? square : null;
	}

	// A new position means whatever was half-clicked is stale.
	$effect(() => {
		void game.game_seq;
		void game.ply;
		untrack(() => (selected = null));
	});

	function label(square: number): string {
		const piece = pieces.find((p) => p.square === square);
		const name = squareName(square);
		if (maskArmed) return `${name}, mask this sensor`;
		return piece ? `${name}, ${piece.letter}` : name;
	}
</script>

<div class="board" class:armed={maskArmed}>
	{#each CELLS as cell (cell.square)}
		<div
			class="sq"
			class:dark={cell.dark}
			class:sel={selected === cell.square}
			class:last={lastMove?.[0] === cell.square || lastMove?.[1] === cell.square}
			class:missing={missing.has(cell.square)}
			class:extra={extra.has(cell.square)}
			class:unknown={unknown.has(cell.square)}
			class:mismatch={mismatch.has(cell.square)}
			class:candidate={candidates.has(cell.square)}
			class:nudge-to={game.detect.nudge?.expected === cell.square}
			class:nudge-from={game.detect.nudge?.actual === cell.square}
			class:masked={masked.has(cell.square)}
			style="--x:{xOf(cell.file)};--y:{yOf(cell.rank)}"
		>
			{#if yOf(cell.rank) === 7}<span class="coord file">{FILES[cell.file]}</span>{/if}
			{#if cell.file === 0}<span class="coord rank">{cell.rank + 1}</span>{/if}
		</div>
	{/each}

	{#each pieces as piece (piece.id)}
		<div
			class="piece"
			class:fresh={piece.fresh}
			style="--x:{xOf(piece.square % 8)};--y:{yOf(Math.floor(piece.square / 8))}"
		>
			<img src={source(piece.letter)} alt="" draggable="false" />
		</div>
	{/each}
	{#each ghosts as ghost (ghost.id)}
		<div
			class="piece ghost"
			style="--x:{xOf(ghost.square % 8)};--y:{yOf(Math.floor(ghost.square / 8))}"
		>
			<img src={source(ghost.letter)} alt="" draggable="false" />
		</div>
	{/each}

	{#each targets as square (square)}
		<div
			class="mark"
			class:ring={occupied.has(square)}
			style="--x:{xOf(square % 8)};--y:{yOf(Math.floor(square / 8))}"
		>
			{#if !occupied.has(square)}<span class="dot"></span>{/if}
		</div>
	{/each}

	{#each CELLS as cell (cell.square)}
		<button
			class="hit"
			class:grab={interactive && movable.has(cell.square)}
			class:point={interactive && targets.includes(cell.square)}
			style="--x:{xOf(cell.file)};--y:{yOf(cell.rank)}"
			disabled={!interactive && !maskArmed}
			aria-label={label(cell.square)}
			onclick={() => onSquare(cell.square)}
		></button>
	{/each}
</div>

<style>
	/* The board carries its own palette. The chrome's near-black square tokens
	   are tuned for the sensor view on `/`, where a square is a readout; here
	   the geometry itself has to be legible from the back of a room. */
	.board {
		--board-light: #e6d7b4;
		--board-dark: #8a6246;
		position: relative;
		width: min(78vmin, 620px);
		aspect-ratio: 1;
		user-select: none;
		border-radius: 10px;
		overflow: hidden;
		box-shadow:
			0 1px 0 rgb(255 255 255 / 0.07) inset,
			0 24px 60px -18px rgb(0 0 0 / 0.7),
			0 0 0 1px rgb(0 0 0 / 0.55);
	}
	.board.armed {
		outline: 2px solid var(--color-warn);
		outline-offset: 3px;
	}

	.sq,
	.piece,
	.mark,
	.hit {
		position: absolute;
		top: 0;
		left: 0;
		width: 12.5%;
		height: 12.5%;
		transform: translate(calc(var(--x) * 100%), calc(var(--y) * 100%));
	}

	.sq {
		background: var(--board-light);
	}
	.sq.dark {
		background: var(--board-dark);
	}
	/* Overlays ride on a pseudo-element so a square can be tinted without
	   disturbing the piece sitting on it. */
	.sq::before {
		content: '';
		position: absolute;
		inset: 0;
		pointer-events: none;
	}

	.sq.last::before {
		background: rgb(90 169 214 / 0.34);
	}
	.sq.sel::before {
		background: rgb(90 169 214 / 0.4);
		box-shadow: inset 0 0 0 3px var(--color-probe);
	}

	/* Setup guidance mirrors the board's own language: amber = still needed,
	   red = take it off, grey = this sensor cannot say. */
	.sq.missing::before {
		background: rgb(214 163 92 / 0.45);
		box-shadow: inset 0 0 0 4px var(--color-warn);
		animation: breathe 1.6s ease-in-out infinite;
	}
	.sq.extra::before {
		background: rgb(207 106 95 / 0.4);
		box-shadow: inset 0 0 0 4px var(--color-fault);
	}
	.sq.unknown::before {
		box-shadow: inset 0 0 0 4px var(--color-fg-faint);
	}
	.sq.mismatch::before {
		background: rgb(207 106 95 / 0.45);
		box-shadow: inset 0 0 0 4px var(--color-fault);
		animation: breathe 0.9s ease-in-out infinite;
	}
	.sq.candidate::before {
		box-shadow: inset 0 0 0 4px var(--color-probe);
		animation: breathe 1.2s ease-in-out infinite;
	}
	.sq.nudge-to::before {
		background: rgb(214 163 92 / 0.45);
		box-shadow: inset 0 0 0 4px var(--color-warn);
		animation: breathe 1s ease-in-out infinite;
	}
	.sq.nudge-from::before {
		box-shadow: inset 0 0 0 4px rgb(207 106 95 / 0.75);
	}
	/* A masked sensor is hatched: the game is deliberately not listening here,
	   and that should be visible rather than implied. */
	.sq.masked::before {
		background: repeating-linear-gradient(45deg, transparent 0 5px, rgb(0 0 0 / 0.42) 5px 10px);
	}

	@keyframes breathe {
		0%,
		100% {
			opacity: 1;
		}
		50% {
			opacity: 0.45;
		}
	}

	.coord {
		position: absolute;
		font-family: var(--font-mono);
		font-size: clamp(8px, 1.3vw, 11px);
		font-weight: 600;
		opacity: 0.7;
		pointer-events: none;
		color: var(--board-dark);
	}
	.sq.dark .coord {
		color: var(--board-light);
	}
	.coord.file {
		right: 6%;
		bottom: 3%;
	}
	.coord.rank {
		left: 6%;
		top: 4%;
	}

	.piece {
		z-index: 2;
		pointer-events: none;
		transition: transform 0.3s cubic-bezier(0.2, 0.85, 0.3, 1);
	}
	.piece img {
		width: 100%;
		height: 100%;
		display: block;
		filter: drop-shadow(0 2px 2px rgb(20 12 5 / 0.45));
	}
	.piece.fresh img {
		animation: pop 0.3s cubic-bezier(0.2, 0.9, 0.35, 1.3) backwards;
	}
	@keyframes pop {
		from {
			opacity: 0;
			transform: scale(0.4);
		}
	}
	.piece.ghost {
		z-index: 1;
		animation: fade 0.3s forwards;
	}
	@keyframes fade {
		to {
			opacity: 0;
			transform: translate(calc(var(--x) * 100%), calc(var(--y) * 100%)) scale(0.7);
		}
	}

	.mark {
		z-index: 3;
		pointer-events: none;
		display: grid;
		place-items: center;
	}
	.mark .dot {
		width: 32%;
		height: 32%;
		border-radius: 50%;
		background: rgb(20 26 30 / 0.4);
	}
	.mark.ring::before {
		content: '';
		position: absolute;
		inset: 4%;
		border-radius: 50%;
		border: 4px solid rgb(20 26 30 / 0.4);
	}

	.hit {
		z-index: 4;
		background: none;
		border: 0;
		padding: 0;
		appearance: none;
		cursor: default;
		-webkit-tap-highlight-color: transparent;
	}
	.hit.grab,
	.hit.point {
		cursor: pointer;
	}
	.hit:focus-visible {
		outline: 2px solid var(--color-probe);
		outline-offset: -2px;
	}

	@media (prefers-reduced-motion: reduce) {
		.piece {
			transition: none;
		}
		.piece.fresh img,
		.piece.ghost {
			animation: none;
		}
		.sq::before {
			animation: none !important;
		}
	}
</style>
