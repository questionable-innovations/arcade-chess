<script lang="ts">
	// The chess-oriented board. `$lib/Board.svelte` stays sensor-oriented and
	// untouched — the diagnostics dashboard is the recovery tool when the
	// electronics act up mid-demo, and it must not regress.
	//
	// Pieces are absolutely positioned and carry stable ids across FEN changes,
	// so a detected move *glides*. On a board where moves normally arrive from
	// sensors rather than from a hand on the mouse, that animation is the thing
	// that tells the room the board was read at all.
	//
	// Input is drag-or-click, through one pointer-event path so mouse and touch
	// behave identically. Manual entry is not a hidden fallback: it is the same
	// UI whether detection is on or off. Detection just does the dragging.

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
		/** Admin-only: moving a piece commits a move. */
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
	const xOf = (square: number) => square % 8;
	const yOf = (square: number) => 7 - Math.floor(square / 8);

	let selected = $state<number | null>(null);
	let hovered = $state<number | null>(null);

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
	// Match the pieces already on screen to the new FEN: same square first, then
	// nearest of the same kind. That covers moves, captures, castling and en
	// passant without any of them being a special case. Leftovers fade out.

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

	function retire(piece: Placed) {
		const ghost = { ...piece };
		ghosts = [...ghosts, ghost];
		setTimeout(() => (ghosts = ghosts.filter((g) => g.id !== ghost.id)), 320);
	}

	function sync(fen: string) {
		// Plain arrays rather than Map/Set: this is function-local scratch, and
		// 64 slots indexed by square is what the rest of this codebase means by
		// "a board".
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
			if (!claimed.includes(piece.id)) retire(piece);
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

	// ── Moving ──────────────────────────────────────────────────────────────

	function commit(from: number, to: number, fromDrag = false) {
		const uci = pickMove(game.legal_moves, from, to);
		if (!uci) return false;
		if (fromDrag) {
			// Settle the dragged piece on its destination immediately, so it does
			// not snap home and glide back when the new state arrives a moment
			// later. Over a WebSocket that round trip is visible.
			const victim = pieces.find((p) => p.square === to);
			if (victim) retire(victim);
			pieces = pieces
				.filter((p) => p.id !== victim?.id)
				.map((p) => (p.square === from ? { ...p, square: to } : p));
		}
		onMove?.(uci);
		selected = null;
		return true;
	}

	// ── Dragging ────────────────────────────────────────────────────────────
	// Pointer events give mouse and touch one path. A drag under the threshold
	// is a click, so tapping a piece then tapping a square still works — which
	// is what an operator on a phone will actually do.

	interface Drag {
		from: number;
		pointerId: number;
		/** Where the pointer went down, board-relative px, for the threshold. */
		startX: number;
		startY: number;
		x: number;
		y: number;
		moved: boolean;
		wasSelected: boolean;
	}
	let boardEl = $state<HTMLElement | null>(null);
	let drag = $state<Drag | null>(null);
	let dragOver = $state<number | null>(null);

	const cellPx = () => (boardEl ? boardEl.getBoundingClientRect().width / 8 : 0);

	function boardPoint(event: PointerEvent): { x: number; y: number } | null {
		if (!boardEl) return null;
		const box = boardEl.getBoundingClientRect();
		return { x: event.clientX - box.left, y: event.clientY - box.top };
	}

	function squareAt(x: number, y: number): number | null {
		const size = cellPx();
		if (!size) return null;
		const file = Math.floor(x / size);
		const row = Math.floor(y / size);
		if (file < 0 || file > 7 || row < 0 || row > 7) return null;
		return (7 - row) * 8 + file;
	}

	function pointerDown(event: PointerEvent, square: number) {
		if (maskArmed) {
			onMaskSquare?.(square);
			return;
		}
		if (!interactive) return;
		if (event.pointerType === 'mouse' && event.button !== 0) return;
		if (selected != null && targets.includes(square)) {
			commit(selected, square);
			return;
		}
		if (!movable.has(square)) {
			selected = null;
			return;
		}
		const point = boardPoint(event);
		if (!point) return;
		const wasSelected = selected === square;
		selected = square;
		drag = {
			from: square,
			pointerId: event.pointerId,
			startX: point.x,
			startY: point.y,
			x: point.x,
			y: point.y,
			moved: false,
			wasSelected
		};
		try {
			(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
		} catch {
			// A synthetic or already-released pointer still works via bubbling.
		}
		event.preventDefault();
	}

	function pointerMove(event: PointerEvent) {
		if (!drag || event.pointerId !== drag.pointerId) return;
		const point = boardPoint(event);
		if (!point) return;
		const moved = drag.moved || Math.hypot(point.x - drag.startX, point.y - drag.startY) > 6;
		drag = { ...drag, x: point.x, y: point.y, moved };
		dragOver = moved ? squareAt(point.x, point.y) : null;
	}

	function pointerUp(event: PointerEvent) {
		if (!drag || event.pointerId !== drag.pointerId) return;
		const finished = drag;
		drag = null;
		dragOver = null;
		if (finished.moved) {
			const point = boardPoint(event);
			const square = point ? squareAt(point.x, point.y) : null;
			// An invalid drop snaps back but keeps the selection for a second try.
			if (square != null && targets.includes(square)) commit(finished.from, square, true);
		} else if (finished.wasSelected) {
			selected = null; // tapping the selected piece again deselects
		}
	}

	function pointerCancel(event: PointerEvent) {
		if (drag && event.pointerId === drag.pointerId) {
			drag = null;
			dragOver = null;
		}
	}

	/** Keyboard path: Enter or Space on a focused square. */
	function keyActivate(square: number) {
		if (maskArmed) {
			onMaskSquare?.(square);
			return;
		}
		if (!interactive) return;
		if (selected != null && targets.includes(square)) {
			commit(selected, square);
			return;
		}
		selected = movable.has(square) && selected !== square ? square : null;
	}

	// A new position means whatever was half-selected is stale.
	$effect(() => {
		void game.game_seq;
		void game.ply;
		untrack(() => {
			selected = null;
			drag = null;
			dragOver = null;
		});
	});

	function label(square: number): string {
		const name = squareName(square);
		if (maskArmed) return `${name}, mask this sensor`;
		const piece = pieces.find((p) => p.square === square);
		let text = piece ? `${name}, ${piece.letter}` : name;
		if (!interactive) return text;
		if (targets.includes(square)) text += '. Move here';
		else if (movable.has(square)) text += selected === square ? '. Selected' : '. Can move';
		return text;
	}
</script>

<div
	class="board"
	class:armed={maskArmed}
	class:live={interactive}
	class:grabbing={drag?.moved}
	bind:this={boardEl}
>
	{#each CELLS as cell (cell.square)}
		<div
			class="sq"
			class:dark={cell.dark}
			class:sel={selected === cell.square}
			class:last={lastMove?.[0] === cell.square || lastMove?.[1] === cell.square}
			class:hot={hovered === cell.square && interactive && movable.has(cell.square)}
			class:dragover={dragOver === cell.square && targets.includes(cell.square)}
			class:missing={missing.has(cell.square)}
			class:extra={extra.has(cell.square)}
			class:unknown={unknown.has(cell.square)}
			class:mismatch={mismatch.has(cell.square)}
			class:candidate={candidates.has(cell.square)}
			class:nudge-to={game.detect.nudge?.expected === cell.square}
			class:nudge-from={game.detect.nudge?.actual === cell.square}
			class:masked={masked.has(cell.square)}
			style="--x:{xOf(cell.square)};--y:{yOf(cell.square)}"
		>
			{#if cell.rank === 0}<span class="coord file">{FILES[cell.file]}</span>{/if}
			{#if cell.file === 0}<span class="coord rank">{cell.rank + 1}</span>{/if}
		</div>
	{/each}

	{#each pieces as piece (piece.id)}
		{@const dragged = drag !== null && drag.moved && drag.from === piece.square}
		<div
			class="piece"
			class:fresh={piece.fresh}
			class:lift={hovered === piece.square && interactive && movable.has(piece.square)}
			class:dragged
			style={dragged && drag
				? `transform: translate(${drag.x - cellPx() / 2}px, ${drag.y - cellPx() / 2}px)`
				: `--x:${xOf(piece.square)};--y:${yOf(piece.square)}`}
		>
			<img src={source(piece.letter)} alt="" draggable="false" />
		</div>
	{/each}
	{#each ghosts as ghost (ghost.id)}
		<div class="piece ghost" style="--x:{xOf(ghost.square)};--y:{yOf(ghost.square)}">
			<img src={source(ghost.letter)} alt="" draggable="false" />
		</div>
	{/each}

	{#each targets as square (square)}
		<div class="mark" class:ring={occupied.has(square)} style="--x:{xOf(square)};--y:{yOf(square)}">
			{#if !occupied.has(square)}<span class="dot"></span>{/if}
		</div>
	{/each}

	{#each CELLS as cell (cell.square)}
		<button
			class="hit"
			class:grab={interactive && movable.has(cell.square)}
			class:point={interactive && targets.includes(cell.square)}
			style="--x:{xOf(cell.square)};--y:{yOf(cell.square)}"
			aria-label={label(cell.square)}
			onpointerdown={(e) => pointerDown(e, cell.square)}
			onpointermove={pointerMove}
			onpointerup={pointerUp}
			onpointercancel={pointerCancel}
			onclick={(e) => {
				// Pointer handlers own mouse and touch; only keyboard "clicks" land
				// here, and those report a detail of 0.
				if (e.detail === 0) keyActivate(cell.square);
			}}
			onmouseenter={() => (hovered = cell.square)}
			onmouseleave={() => (hovered = null)}
		></button>
	{/each}
</div>

<style>
	.board {
		position: relative;
		width: min(78vmin, 620px);
		aspect-ratio: 1;
		user-select: none;
		border-radius: 10px;
		overflow: hidden;
		border: 1px solid var(--color-line);
		box-shadow: 0 18px 60px rgb(0 0 0 / 0.45);
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
		background: var(--color-sq-light);
	}
	.sq.dark {
		background: var(--color-sq-dark);
	}
	/* Overlays ride on a pseudo-element so a square can be tinted without
	   disturbing the piece sitting on it. */
	.sq::before {
		content: '';
		position: absolute;
		inset: 0;
		pointer-events: none;
		transition: box-shadow 0.12s ease;
	}

	.sq.last::before {
		background: color-mix(in srgb, var(--color-probe) 22%, transparent);
	}
	.sq.sel::before {
		background: color-mix(in srgb, var(--color-probe) 26%, transparent);
		box-shadow: inset 0 0 0 3px var(--color-probe);
	}
	.sq.hot::before {
		box-shadow: inset 0 0 0 2px color-mix(in srgb, var(--color-probe) 45%, transparent);
	}
	.sq.dragover::before {
		background: color-mix(in srgb, var(--color-probe) 30%, transparent);
		box-shadow: inset 0 0 0 4px var(--color-probe);
	}

	/* Setup guidance mirrors the board's own language: amber = still needed,
	   red = take it off, grey = this sensor cannot say. */
	.sq.missing::before {
		background: color-mix(in srgb, var(--color-warn) 26%, transparent);
		box-shadow: inset 0 0 0 4px var(--color-warn);
		animation: breathe 1.6s ease-in-out infinite;
	}
	.sq.extra::before {
		background: color-mix(in srgb, var(--color-fault) 24%, transparent);
		box-shadow: inset 0 0 0 4px var(--color-fault);
	}
	.sq.unknown::before {
		box-shadow: inset 0 0 0 4px var(--color-fg-faint);
	}
	.sq.mismatch::before {
		background: color-mix(in srgb, var(--color-fault) 26%, transparent);
		box-shadow: inset 0 0 0 4px var(--color-fault);
		animation: breathe 0.9s ease-in-out infinite;
	}
	.sq.candidate::before {
		box-shadow: inset 0 0 0 4px var(--color-probe);
		animation: breathe 1.2s ease-in-out infinite;
	}
	.sq.nudge-to::before {
		background: color-mix(in srgb, var(--color-warn) 26%, transparent);
		box-shadow: inset 0 0 0 4px var(--color-warn);
		animation: breathe 1s ease-in-out infinite;
	}
	.sq.nudge-from::before {
		box-shadow: inset 0 0 0 4px color-mix(in srgb, var(--color-fault) 70%, transparent);
	}
	/* A masked sensor is hatched: the game is deliberately not listening here,
	   and that should be visible rather than implied. */
	.sq.masked::before {
		background: repeating-linear-gradient(45deg, transparent 0 5px, rgb(0 0 0 / 0.4) 5px 10px);
	}

	@keyframes breathe {
		0%,
		100% {
			opacity: 1;
		}
		50% {
			opacity: 0.4;
		}
	}

	.coord {
		position: absolute;
		font-family: var(--font-mono);
		font-size: clamp(8px, 1.3vw, 11px);
		font-weight: 600;
		color: var(--color-fg-faint);
		pointer-events: none;
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
		/* The SVGs carry their own outline, so a black piece keeps its silhouette
		   on a near-black square where a Unicode glyph would disappear. */
		filter: drop-shadow(0 2px 3px rgb(0 0 0 / 0.55));
		transition:
			transform 0.15s ease,
			filter 0.2s ease;
	}
	.piece.lift img {
		transform: translateY(-4%) scale(1.05);
	}
	.piece.dragged {
		transition: none;
		z-index: 11;
	}
	.piece.dragged img {
		transform: scale(1.14);
		filter: drop-shadow(0 10px 14px rgb(0 0 0 / 0.6));
		transition: none;
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
		}
	}
	.piece.ghost img {
		animation: shrink 0.3s forwards;
	}
	@keyframes shrink {
		to {
			transform: scale(0.65);
		}
	}

	.mark {
		z-index: 3;
		pointer-events: none;
		display: grid;
		place-items: center;
	}
	.mark .dot {
		width: 30%;
		height: 30%;
		border-radius: 50%;
		background: color-mix(in srgb, var(--color-probe) 70%, transparent);
	}
	.mark.ring::before {
		content: '';
		position: absolute;
		inset: 5%;
		border-radius: 50%;
		border: 4px solid color-mix(in srgb, var(--color-probe) 65%, transparent);
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
	.hit.grab {
		cursor: grab;
	}
	.hit.point {
		cursor: pointer;
	}
	/* While the board takes input it owns touch gestures, so a drag that misses
	   a piece never scrolls the page out from under the move. When it is locked,
	   touches pass straight through to the scroller. */
	.board.live .hit {
		touch-action: none;
	}
	.board.grabbing .hit {
		cursor: grabbing;
	}
	.hit:focus-visible {
		outline: 2px solid var(--color-probe);
		outline-offset: -2px;
	}

	@media (prefers-reduced-motion: reduce) {
		.piece,
		.piece img {
			transition: none;
		}
		.piece.fresh img,
		.piece.ghost,
		.piece.ghost img {
			animation: none;
		}
		.sq::before {
			animation: none !important;
		}
	}
</style>
