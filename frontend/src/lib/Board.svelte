<script lang="ts">
	import { nodeHealth, type Envelope, type SquareState } from './types';

	let {
		squares,
		valid,
		nodeStatus,
		admin,
		onSquare
	}: {
		squares: SquareState[];
		valid: boolean[];
		nodeStatus: (Envelope | null)[];
		admin: boolean;
		onSquare: (i: number) => void;
	} = $props();

	// Visual layout order: rank 8 on top, so quadrants read TL, TR, BL, BR.
	const quadOrder = [2, 3, 0, 1];

	function quadSquares(node: number): number[] {
		const rLo = node < 2 ? 0 : 4;
		const cLo = node % 2 === 0 ? 0 : 4;
		const out: number[] = [];
		for (let r = rLo + 3; r >= rLo; r--) {
			for (let c = cLo; c <= cLo + 3; c++) out.push(r * 8 + c);
		}
		return out;
	}

	const files = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'];
	const ranks = ['8', '7', '6', '5', '4', '3', '2', '1'];
</script>

<div class="frame">
	<div class="ranks">
		{#each ranks as r (r)}<span>{r}</span>{/each}
	</div>
	<div class="board">
		{#each quadOrder as node (node)}
			<div class="quad {nodeHealth(nodeStatus[node])}">
				<span class="tag">n{node}</span>
				{#each quadSquares(node) as i (i)}
					{@const dark = (Math.floor(i / 8) + (i % 8)) % 2 === 0}
					<button
						class="sq {dark ? 'dark' : 'light'}"
						onclick={() => onSquare(i)}
						disabled={!admin}
						aria-label={`square ${i}`}
					>
						<span class="glow {squares[i]}"></span>
						{#if !valid[i]}
							<span class="dot"></span>
						{/if}
					</button>
				{/each}
			</div>
		{/each}
	</div>
	<div class="corner"></div>
	<div class="files">
		{#each files as f (f)}<span>{f}</span>{/each}
	</div>
</div>

<style>
	.frame {
		display: grid;
		grid-template-columns: 1.2em 1fr;
		grid-template-rows: 1fr 1.2em;
		grid-template-areas:
			'ranks board'
			'corner files';
		width: min(76vh, 92vw);
		gap: 2px;
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
		font-size: 10px;
		line-height: 1;
		color: #6e6e78;
	}
	.corner {
		grid-area: corner;
	}

	.board {
		grid-area: board;
		display: grid;
		grid-template-columns: 1fr 1fr;
		grid-template-rows: 1fr 1fr;
		gap: 3px;
		aspect-ratio: 1;
		width: 100%;
		background: #060607;
	}

	.quad {
		position: relative;
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		grid-template-rows: repeat(4, 1fr);
		outline: 1px solid transparent;
		outline-offset: -1px;
	}
	.quad .tag {
		position: absolute;
		top: 3px;
		left: 4px;
		font-size: 9px;
		line-height: 1;
		z-index: 1;
		pointer-events: none;
		color: rgba(140, 140, 150, 0.55);
	}
	.quad.healthy {
		outline-color: rgba(34, 197, 94, 0.4);
	}
	.quad.healthy .tag {
		color: rgba(34, 197, 94, 0.65);
	}
	.quad.uncalibrated {
		outline-color: rgba(245, 158, 11, 0.55);
	}
	.quad.uncalibrated .tag {
		color: rgba(245, 158, 11, 0.8);
	}
	.quad.uncalibrated::after {
		content: '';
		position: absolute;
		inset: 0;
		background: rgba(245, 158, 11, 0.08);
		pointer-events: none;
	}
	.quad.offline {
		outline-color: rgba(239, 68, 68, 0.55);
		opacity: 0.32;
	}
	.quad.offline .tag {
		color: rgba(239, 68, 68, 0.9);
	}
	.quad.unseen {
		outline-color: rgba(140, 140, 150, 0.35);
		opacity: 0.45;
	}

	.sq {
		position: relative;
		overflow: hidden;
		border: 0;
		margin: 0;
		padding: 0;
		font: inherit;
		cursor: default;
	}
	.sq.dark {
		background: #17171b;
	}
	.sq.light {
		background: #2e2e36;
	}
	.sq:not(:disabled) {
		cursor: pointer;
	}
	.sq:not(:disabled):hover {
		box-shadow: inset 0 0 0 1px rgba(0, 160, 255, 0.6);
	}

	/* The physical board lights the whole square from below; mirror that with a
	   soft radial wash instead of a piece glyph. */
	.glow {
		position: absolute;
		inset: 0;
		opacity: 0;
		pointer-events: none;
		transition: opacity 0.3s ease;
	}
	.glow.positive {
		opacity: 1;
		background: radial-gradient(
			circle at 50% 50%,
			rgba(74, 222, 128, 0.85) 0%,
			rgba(34, 197, 94, 0.38) 55%,
			rgba(34, 197, 94, 0.14) 100%
		);
	}
	.glow.negative {
		opacity: 1;
		background: radial-gradient(
			circle at 50% 50%,
			rgba(96, 165, 250, 0.85) 0%,
			rgba(59, 130, 246, 0.38) 55%,
			rgba(59, 130, 246, 0.14) 100%
		);
	}
	.glow.uncertain {
		opacity: 1;
		background: radial-gradient(
			circle at 50% 50%,
			rgba(228, 228, 235, 0.4) 0%,
			rgba(228, 228, 235, 0.12) 60%,
			transparent 100%
		);
	}

	.dot {
		position: relative;
		display: block;
		margin: auto;
		width: 14%;
		aspect-ratio: 1;
		border-radius: 50%;
		background: rgba(170, 170, 180, 0.5);
	}

	.sq {
		display: flex;
		align-items: center;
		justify-content: center;
	}

	@media (prefers-reduced-motion: reduce) {
		.glow {
			transition: none;
		}
	}
</style>
