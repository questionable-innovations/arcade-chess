<script lang="ts">
	// The chess-oriented board. `$lib/Board.svelte` stays sensor-oriented and
	// untouched — the bring-up dashboard is the recovery tool when the
	// electronics act up mid-demo, and it must not regress.
	//
	// Manual move entry is not a hidden fallback: this is the same click-to-move
	// UI whether detection is on or off. Detection just does the clicking.

	import {
		boardFromFen,
		DISPLAY_ORDER,
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

	let selected = $state<number | null>(null);

	const pieces = $derived(boardFromFen(game.fen));
	const targets = $derived(selected == null ? [] : targetsFrom(game.legal_moves, selected));
	const movable = $derived(
		new Set(game.legal_moves.map((uci) => uciSquares(uci)?.[0]).filter((v) => v != null))
	);

	const lastMove = $derived.by(() => {
		const last = game.moves.at(-1);
		return last ? uciSquares(last.uci) : null;
	});

	const setupTargets = $derived.by(() => {
		if (game.phase !== 'setup' && game.phase !== 'countdown') return null;
		return {
			missing: new Set(game.setup?.missing ?? []),
			extra: new Set(game.setup?.extra ?? []),
			unknown: new Set(game.setup?.unknown ?? [])
		};
	});

	const mismatch = $derived(new Set(game.detect.mismatch));
	const masked = $derived(new Set(game.detect.masked));
	const candidates = $derived(
		new Set(
			(game.choice?.options ?? [])
				.map((option) => uciSquares(option.uci)?.[1])
				.filter((square) => square != null)
		)
	);

	// During setup the board shows the position to *build*, not the position in
	// play — they are the same until the first move, but saying so explicitly
	// keeps the countdown honest.
	const shown = $derived(
		game.phase === 'setup' || game.phase === 'countdown' ? boardFromFen(game.start_fen) : pieces
	);

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
</script>

<div class="board" class:armed={maskArmed}>
	{#each DISPLAY_ORDER as square (square)}
		{@const piece = shown[square]}
		{@const dark = (Math.floor(square / 8) + (square % 8)) % 2 === 0}
		<button
			class="sq"
			class:dark
			class:light={!dark}
			class:selected={selected === square}
			class:target={targets.includes(square)}
			class:from={lastMove?.[0] === square}
			class:to={lastMove?.[1] === square}
			class:missing={setupTargets?.missing.has(square)}
			class:extra={setupTargets?.extra.has(square)}
			class:unknown={setupTargets?.unknown.has(square)}
			class:mismatch={mismatch.has(square)}
			class:candidate={candidates.has(square)}
			class:nudge-to={game.detect.nudge?.expected === square}
			class:nudge-from={game.detect.nudge?.actual === square}
			class:masked={masked.has(square)}
			disabled={!interactive && !maskArmed}
			aria-label={squareName(square)}
			onclick={() => onSquare(square)}
		>
			{#if piece}
				<span class="piece" class:white={piece.white} class:black={!piece.white}>
					{piece.glyph}
				</span>
			{/if}
			{#if targets.includes(square) && !piece}
				<span class="dot"></span>
			{/if}
			{#if square % 8 === 0}<span class="rank">{Math.floor(square / 8) + 1}</span>{/if}
			{#if square < 8}<span class="file">{squareName(square)[0]}</span>{/if}
		</button>
	{/each}
</div>

<style>
	.board {
		display: grid;
		grid-template-columns: repeat(8, 1fr);
		width: min(78vmin, 640px);
		aspect-ratio: 1;
		border-radius: 10px;
		overflow: hidden;
		border: 1px solid var(--color-line);
		box-shadow: 0 18px 60px rgb(0 0 0 / 0.45);
	}
	.board.armed {
		outline: 2px solid var(--color-warn);
		outline-offset: 3px;
	}

	.sq {
		position: relative;
		display: grid;
		place-items: center;
		border: 0;
		padding: 0;
		cursor: default;
		font-size: min(8.4vmin, 68px);
		line-height: 1;
		transition: box-shadow 0.12s ease;
	}
	.sq:not(:disabled) {
		cursor: pointer;
	}
	.dark {
		background: var(--color-sq-dark);
	}
	.light {
		background: var(--color-sq-light);
	}

	.piece {
		/* One glyph, two fills: the outline forms are unreadable at a distance,
		   so both colours use the solid glyph and differ by fill. */
		user-select: none;
		pointer-events: none;
	}
	.piece.white {
		color: #f4f6f8;
		text-shadow:
			0 0 2px #000,
			0 2px 3px rgb(0 0 0 / 0.65);
	}
	.piece.black {
		color: #14161a;
		text-shadow:
			0 0 2px rgb(255 255 255 / 0.55),
			0 1px 2px rgb(0 0 0 / 0.4);
	}

	.dot {
		width: 22%;
		height: 22%;
		border-radius: 50%;
		background: color-mix(in srgb, var(--color-probe) 70%, transparent);
	}

	.selected {
		box-shadow: inset 0 0 0 3px var(--color-probe);
	}
	.target {
		box-shadow: inset 0 0 0 3px color-mix(in srgb, var(--color-probe) 55%, transparent);
	}
	.from,
	.to {
		box-shadow: inset 0 0 0 3px color-mix(in srgb, var(--color-probe) 80%, transparent);
	}

	/* Setup guidance mirrors the board's own language: amber = still needed. */
	.missing {
		box-shadow: inset 0 0 0 4px var(--color-warn);
		animation: breathe 1.6s ease-in-out infinite;
	}
	.extra {
		box-shadow: inset 0 0 0 4px var(--color-fault);
	}
	.unknown {
		box-shadow: inset 0 0 0 4px var(--color-fg-faint);
	}
	.mismatch {
		box-shadow: inset 0 0 0 4px var(--color-fault);
		animation: breathe 0.9s ease-in-out infinite;
	}
	.candidate {
		box-shadow: inset 0 0 0 4px var(--color-probe);
		animation: breathe 1.2s ease-in-out infinite;
	}
	.nudge-to {
		box-shadow: inset 0 0 0 4px var(--color-warn);
		animation: breathe 1s ease-in-out infinite;
	}
	.nudge-from {
		box-shadow: inset 0 0 0 4px color-mix(in srgb, var(--color-fault) 70%, transparent);
	}
	.masked::after {
		content: '';
		position: absolute;
		inset: 0;
		background: repeating-linear-gradient(45deg, transparent 0 6px, rgb(0 0 0 / 0.35) 6px 12px);
		pointer-events: none;
	}

	@keyframes breathe {
		0%,
		100% {
			filter: brightness(1);
		}
		50% {
			filter: brightness(1.45);
		}
	}

	.rank,
	.file {
		position: absolute;
		font-family: var(--font-mono);
		font-size: 10px;
		color: var(--color-fg-faint);
		pointer-events: none;
	}
	.rank {
		top: 3px;
		left: 4px;
	}
	.file {
		bottom: 2px;
		right: 4px;
	}
</style>
