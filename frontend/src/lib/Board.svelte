<script lang="ts">
	import {
		adcToVolts,
		nodeHealth,
		polarityColor,
		ADC_CENTER,
		NODE_INDICES,
		type Envelope,
		type SquareState
	} from './types';

	let {
		squares,
		valid,
		nodeStatus,
		rawAdc = null,
		baselineAdc = null,
		debug = false,
		heatmap = false,
		heatSpan = 512,
		stale = null,
		admin = false,
		probeFlash = null,
		onSquare
	}: {
		squares: SquareState[];
		valid: boolean[];
		nodeStatus: (Envelope | null)[];
		rawAdc?: (number | null)[] | null;
		baselineAdc?: (number | null)[] | null;
		debug?: boolean;
		heatmap?: boolean;
		heatSpan?: number;
		/** Why the rendered position can no longer be trusted; null when live. */
		stale?: string | null;
		admin?: boolean;
		probeFlash?: number | null;
		onSquare: (i: number) => void;
	} = $props();

	const heatOn = $derived(debug && heatmap && !!rawAdc);

	// Signed deviation from the square's centre (its baseline, else the mid-rail),
	// in raw ADC counts. Positive = above centre, negative = below.
	function deviation(i: number): number | null {
		const adc = rawAdc?.[i];
		if (adc == null) return null;
		const centre = baselineAdc?.[i] ?? ADC_CENTER;
		return adc - centre;
	}

	// Normalised polarity in [-1, +1] for the diverging colour ramp.
	function heatT(i: number): number | null {
		const d = deviation(i);
		if (d == null) return null;
		return heatSpan > 0 ? d / heatSpan : 0;
	}

	// Cell fill in heatmap mode; undefined lets the state/base CSS win otherwise.
	function fillOf(i: number): string | undefined {
		if (!heatOn) return undefined;
		const t = heatT(i);
		return t == null ? undefined : polarityColor(t);
	}

	// Keep the overlaid number legible against a saturated pole.
	function inkOf(i: number): string | undefined {
		if (!heatOn) return undefined;
		const t = heatT(i);
		if (t == null) return undefined;
		return Math.abs(t) > 0.62 ? 'rgba(9, 13, 16, 0.9)' : 'rgba(233, 236, 238, 0.86)';
	}

	// Signed deviation voltage (± around centre) — the meaningful readout here.
	function devVolts(i: number): number | null {
		return adcToVolts(deviation(i));
	}

	function voltLabel(i: number): string {
		const v = devVolts(i);
		if (v == null) return '—';
		const s = v >= 0 ? '+' : '−';
		return `${s}${Math.abs(v).toFixed(2)}`;
	}

	// Default (orientation 0) inverse of the ESP's globalSquare(): which node and
	// local sensor index feed this global square — the first thing to check when
	// a reading lands on the wrong square.
	function localOf(i: number): number {
		const r = Math.floor(i / 8);
		const c = i % 8;
		return (r % 4) * 4 + (c % 4);
	}

	function cellTitle(i: number, file: string, rank: number, node: number): string {
		const base = `${file}${rank} · n${node} local ${localOf(i)}`;
		const v = devVolts(i);
		if (v == null) return base;
		const s = v >= 0 ? '+' : '−';
		const noise = baselineAdc ? ` base ${baselineAdc[i] ?? '—'}` : '';
		return `${base} · ${s}${Math.abs(v).toFixed(3)} V · adc ${rawAdc?.[i]}${noise}`;
	}

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
</script>

<div class="wrap" class:debug>
	<div class="ranks" aria-hidden="true">
		{#each ranks as r (r)}<span>{r}</span>{/each}
	</div>

	<div class="board" class:stale={!!stale} role="grid" aria-label="chess board sensor state">
		{#each cells as cell (cell.i)}
			<button
				class="sq {cell.dark ? 'dark' : 'light'} {squares[cell.i]}"
				class:down={nodeDown(cell.node)}
				class:invalid={!valid[cell.i]}
				class:heat={heatOn}
				class:probed={probeFlash === cell.i}
				style:background={fillOf(cell.i)}
				disabled={!admin}
				onclick={() => onSquare(cell.i)}
				aria-label={`${files[cell.col]}${cell.row + 1} ${squares[cell.i]}`}
				title={debug ? cellTitle(cell.i, files[cell.col], cell.row + 1, cell.node) : undefined}
			>
				{#if debug && rawAdc}
					<span class="volt" style:color={inkOf(cell.i)}>{voltLabel(cell.i)}</span>
				{/if}
				{#if !valid[cell.i]}<span class="flag" title="invalid reading"></span>{/if}
			</button>
		{/each}

		{#if stale}
			<!-- The last known position is still drawn, but never as if it were live. -->
			<div class="stalemark" role="status">
				<span class="word">STALE</span>
				<span class="why">{stale}</span>
			</div>
		{/if}

		{#if debug}
			<!-- Quadrant seams + node badges surface the physical boards only here. -->
			<div class="seam v"></div>
			<div class="seam h"></div>
			{#each NODE_INDICES as node (node)}
				<span class="badge q{node} {nodeHealth(nodeStatus[node])}">n{node}</span>
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

	/* Stale: the panel visibly goes cold so a frozen position can't read live. */
	/* Filter the squares, not the board, so the marker itself stays legible. */
	.board.stale .sq {
		filter: saturate(0.25) brightness(0.72);
	}
	.stalemark {
		position: absolute;
		z-index: 4;
		top: 50%;
		left: 50%;
		transform: translate(-50%, -50%);
		display: flex;
		align-items: baseline;
		gap: 9px;
		padding: 6px 14px;
		font-family: var(--font-mono);
		background: rgba(8, 9, 10, 0.82);
		border: 1px dashed color-mix(in srgb, var(--color-fault) 60%, var(--color-line));
		border-radius: 6px;
		backdrop-filter: blur(4px);
		pointer-events: none;
	}
	.stalemark .word {
		font-size: 11px;
		font-weight: 700;
		letter-spacing: 0.18em;
		color: var(--color-fault);
	}
	.stalemark .why {
		font-size: 10px;
		letter-spacing: 0.04em;
		color: var(--color-fg-faint);
	}

	/* Graceful degradation in normal view: an offline quadrant simply dims. */
	.sq.down {
		opacity: 0.32;
		filter: saturate(0.4);
	}

	/* An invalid reading is marked by a corner wedge rather than a lamp: it is a
	   property of the square, not a device with a state. */
	.flag {
		position: absolute;
		top: 0;
		right: 0;
		width: 0;
		height: 0;
		border-top: 7px solid var(--color-warn);
		border-left: 7px solid transparent;
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

	/* Quadrant identity plus its health in one token — the colour is the state,
	   so there is nothing to put a lamp beside. */
	.badge {
		position: absolute;
		z-index: 3;
		padding: 2px 6px;
		font-family: var(--font-mono);
		font-size: 10px;
		letter-spacing: 0.04em;
		color: var(--color-fg-faint);
		background: rgba(8, 9, 10, 0.72);
		border: 1px solid var(--color-line);
		border-radius: 4px;
		backdrop-filter: blur(4px);
		pointer-events: none;
	}
	.badge.healthy {
		color: var(--color-live);
		border-color: color-mix(in srgb, var(--color-live) 32%, var(--color-line));
	}
	.badge.uncalibrated {
		color: var(--color-warn);
		border-color: color-mix(in srgb, var(--color-warn) 36%, var(--color-line));
	}
	.badge.offline {
		color: var(--color-fault);
		border-color: color-mix(in srgb, var(--color-fault) 40%, var(--color-line));
	}
	/* Badge positions must match the cell→node attribution above: node 0 owns
	   ranks 1-4 files a-d (bottom-left on screen), node 3 the top-right. */
	.q0 {
		bottom: 8px;
		left: 8px;
	}
	.q1 {
		bottom: 8px;
		right: 8px;
	}
	.q2 {
		top: 8px;
		left: 8px;
	}
	.q3 {
		top: 8px;
		right: 8px;
	}

	.sq.probed {
		box-shadow: inset 0 0 0 3px var(--color-probe);
		animation: probe-fade 1.2s ease-out forwards;
	}
	@keyframes probe-fade {
		from {
			box-shadow: inset 0 0 0 3px var(--color-probe);
		}
		to {
			box-shadow: inset 0 0 0 3px transparent;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.sq {
			transition: none;
		}
	}
</style>
