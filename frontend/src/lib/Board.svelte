<script lang="ts">
	import { nodeHealth, adcToVolts, type Envelope, type SquareState } from './types';

	let {
		squares,
		valid,
		nodeStatus,
		rawAdc = null,
		debug = false,
		admin = false,
		onSquare
	}: {
		squares: SquareState[];
		valid: boolean[];
		nodeStatus: (Envelope | null)[];
		rawAdc?: (number | null)[] | null;
		debug?: boolean;
		admin?: boolean;
		onSquare: (i: number) => void;
	} = $props();

	const files = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'];
	const ranks = ['8', '7', '6', '5', '4', '3', '2', '1'];

	// Display order: rank 8 (row 7) on top, file a (col 0) on the left. One
	// continuous 8×8 — the four quadrants are invisible unless debug seams show.
	const cells = $derived.by(() => {
		const out: { i: number; row: number; col: number; node: number; dark: boolean }[] = [];
		for (let r = 7; r >= 0; r--) {
			for (let c = 0; c < 8; c++) {
				const node = (r >= 4 ? 2 : 0) + (c >= 4 ? 1 : 0);
				out.push({ i: r * 8 + c, row: r, col: c, node, dark: (r + c) % 2 === 0 });
			}
		}
		return out;
	});

	// A quadrant is "down" only when its node actually reported offline; an unseen
	// node during bring-up shouldn't grey out an otherwise working board.
	function nodeDown(node: number): boolean {
		return nodeHealth(nodeStatus[node]) === 'offline';
	}

	function voltLabel(i: number): string {
		const v = adcToVolts(rawAdc?.[i]);
		return v == null ? '—' : v.toFixed(2);
	}
</script>

<div class="wrap" class:debug>
	<div class="ranks" aria-hidden="true">
		{#each ranks as r (r)}<span>{r}</span>{/each}
	</div>

	<div class="board" role="grid" aria-label="chess board sensor state">
		{#each cells as cell (cell.i)}
			<button
				class="sq {cell.dark ? 'dark' : 'light'} {squares[cell.i]}"
				class:down={nodeDown(cell.node)}
				class:invalid={!valid[cell.i]}
				disabled={!admin}
				onclick={() => onSquare(cell.i)}
				aria-label={`${files[cell.col]}${cell.row + 1} ${squares[cell.i]}`}
			>
				{#if debug && rawAdc}
					<span class="volt">{voltLabel(cell.i)}</span>
				{/if}
				{#if !valid[cell.i]}<span class="flag" title="invalid reading"></span>{/if}
			</button>
		{/each}

		{#if debug}
			<!-- Quadrant seams + node badges surface the physical boards only here. -->
			<div class="seam v"></div>
			<div class="seam h"></div>
			{#each [0, 1, 2, 3] as node (node)}
				<span class="badge q{node} {nodeHealth(nodeStatus[node])}">
					<span class="ring"></span>n{node}
				</span>
			{/each}
		{/if}
	</div>

	<div class="corner" aria-hidden="true"></div>
	<div class="files" aria-hidden="true">
		{#each files as f (f)}<span>{f}</span>{/each}
	</div>
</div>

<style>
	.wrap {
		display: grid;
		grid-template-columns: 1.4em 1fr;
		grid-template-rows: 1fr 1.4em;
		grid-template-areas:
			'ranks board'
			'corner files';
		width: min(74vh, 92vw);
		gap: 4px;
	}
	.ranks {
		grid-area: ranks;
		display: flex;
		flex-direction: column;
		justify-content: space-around;
		align-items: center;
	}
	.files {
		grid-area: files;
		display: flex;
		justify-content: space-around;
		align-items: center;
	}
	.ranks span,
	.files span {
		font-family: var(--font-mono);
		font-size: 10px;
		letter-spacing: 0.06em;
		color: var(--color-fg-ghost);
	}
	.corner {
		grid-area: corner;
	}

	/* One seamless surface: a single grid, hairline internal lines, a soft
	   perimeter and drop shadow so the whole thing reads as one physical panel. */
	.board {
		grid-area: board;
		position: relative;
		display: grid;
		grid-template-columns: repeat(8, 1fr);
		grid-template-rows: repeat(8, 1fr);
		aspect-ratio: 1;
		width: 100%;
		border-radius: 10px;
		overflow: hidden;
		background: var(--color-line-soft);
		gap: 1px;
		padding: 1px;
		box-shadow:
			0 1px 0 rgba(255, 255, 255, 0.04) inset,
			0 24px 60px -24px rgba(0, 0, 0, 0.8),
			0 0 0 1px var(--color-line);
	}

	.sq {
		position: relative;
		border: 0;
		margin: 0;
		padding: 0;
		font: inherit;
		cursor: default;
		display: flex;
		align-items: center;
		justify-content: center;
		transition:
			background-color 0.28s ease,
			box-shadow 0.15s ease;
	}
	.sq.dark {
		background: var(--color-sq-dark);
	}
	.sq.light {
		background: var(--color-sq-light);
	}
	.sq:not(:disabled) {
		cursor: pointer;
	}
	.sq:not(:disabled):hover {
		box-shadow: inset 0 0 0 2px color-mix(in srgb, var(--color-probe) 65%, transparent);
	}

	/* Piece present → the entire square is filled with a calm, matte signal
	   colour. A whisper of vertical shading keeps it feeling lit, not painted. */
	.sq.positive {
		background:
			linear-gradient(180deg, rgba(255, 255, 255, 0.06), rgba(0, 0, 0, 0.06)), var(--color-pos);
	}
	.sq.negative {
		background:
			linear-gradient(180deg, rgba(255, 255, 255, 0.06), rgba(0, 0, 0, 0.06)), var(--color-neg);
	}
	.sq.uncertain {
		background:
			linear-gradient(180deg, rgba(255, 255, 255, 0.04), rgba(0, 0, 0, 0.05)),
			var(--color-uncertain);
	}

	/* Graceful degradation in normal view: an offline quadrant simply dims. */
	.sq.down {
		opacity: 0.32;
		filter: saturate(0.4);
	}

	.flag {
		position: absolute;
		top: 3px;
		right: 3px;
		width: 5px;
		height: 5px;
		border-radius: 50%;
		background: var(--color-warn);
		opacity: 0.75;
	}

	/* ── Debug layer ─────────────────────────────────────────────────────────*/
	.volt {
		font-family: var(--font-mono);
		font-size: clamp(7px, 1.1vw, 11px);
		font-variant-numeric: tabular-nums;
		color: rgba(233, 236, 238, 0.82);
		text-shadow: 0 1px 2px rgba(0, 0, 0, 0.6);
		pointer-events: none;
	}

	.seam {
		position: absolute;
		background: rgba(90, 169, 214, 0.3);
		pointer-events: none;
		z-index: 2;
	}
	.seam.v {
		top: 0;
		bottom: 0;
		left: 50%;
		width: 1px;
		transform: translateX(-0.5px);
	}
	.seam.h {
		left: 0;
		right: 0;
		top: 50%;
		height: 1px;
		transform: translateY(-0.5px);
	}

	.badge {
		position: absolute;
		z-index: 3;
		display: inline-flex;
		align-items: center;
		gap: 5px;
		padding: 2px 7px;
		font-family: var(--font-mono);
		font-size: 10px;
		letter-spacing: 0.04em;
		color: var(--color-fg-dim);
		background: rgba(8, 9, 10, 0.72);
		border: 1px solid var(--color-line);
		border-radius: 999px;
		backdrop-filter: blur(4px);
		pointer-events: none;
	}
	.badge .ring {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: var(--color-fg-faint);
	}
	.badge.healthy .ring {
		background: var(--color-live);
	}
	.badge.uncalibrated .ring {
		background: var(--color-warn);
	}
	.badge.offline .ring {
		background: var(--color-fault);
	}
	.badge.healthy {
		color: var(--color-fg);
	}
	.q0 {
		top: 8px;
		left: 8px;
	}
	.q1 {
		top: 8px;
		right: 8px;
	}
	.q2 {
		bottom: 8px;
		left: 8px;
	}
	.q3 {
		bottom: 8px;
		right: 8px;
	}

	@media (prefers-reduced-motion: reduce) {
		.sq {
			transition: none;
		}
	}
</style>
