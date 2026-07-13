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
				{#each quadSquares(node) as i (i)}
					{@const dark = (Math.floor(i / 8) + (i % 8)) % 2 === 0}
					<button
						class="sq {dark ? 'dark' : 'light'}"
						onclick={() => onSquare(i)}
						disabled={!admin}
						aria-label={`square ${i}`}
					>
						{#if !valid[i]}
							<span class="dot"></span>
						{:else if squares[i] === 'positive'}
							<span class="disc positive"></span>
						{:else if squares[i] === 'negative'}
							<span class="disc negative"></span>
						{:else if squares[i] === 'uncertain'}
							<span class="ring"></span>
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
		grid-template-columns: 1.1em 1fr;
		grid-template-rows: 1fr 1.1em;
		grid-template-areas:
			'ranks board'
			'corner files';
		width: min(72vh, 90vw);
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
		font-size: 9px;
		line-height: 1;
		color: #55555c;
	}
	.corner {
		grid-area: corner;
	}

	.board {
		grid-area: board;
		display: grid;
		grid-template-columns: 1fr 1fr;
		grid-template-rows: 1fr 1fr;
		gap: 2px;
		aspect-ratio: 1;
		width: 100%;
		background: #0a0a0b;
	}

	.quad {
		position: relative;
		display: grid;
		grid-template-columns: repeat(4, 1fr);
		grid-template-rows: repeat(4, 1fr);
		outline: 1px solid transparent;
		outline-offset: -1px;
	}
	.quad.healthy {
		outline-color: rgba(34, 197, 94, 0.28);
	}
	.quad.uncalibrated {
		outline-color: rgba(245, 158, 11, 0.4);
	}
	.quad.uncalibrated::after {
		content: '';
		position: absolute;
		inset: 0;
		background: rgba(245, 158, 11, 0.07);
		pointer-events: none;
	}
	.quad.offline {
		outline-color: rgba(239, 68, 68, 0.4);
		opacity: 0.35;
	}
	.quad.unseen {
		outline-color: rgba(140, 140, 150, 0.28);
		opacity: 0.5;
	}

	.sq {
		display: flex;
		align-items: center;
		justify-content: center;
		border: 0;
		margin: 0;
		padding: 0;
		font: inherit;
		cursor: default;
	}
	.sq.dark {
		background: #1a1a1c;
	}
	.sq.light {
		background: #232326;
	}
	.sq:not(:disabled) {
		cursor: pointer;
	}
	.sq:not(:disabled):hover {
		box-shadow: inset 0 0 0 1px rgba(0, 160, 255, 0.6);
	}

	.disc {
		width: 55%;
		aspect-ratio: 1;
		border-radius: 50%;
		box-shadow: inset 0 -1px 2px rgba(0, 0, 0, 0.28);
	}
	.disc.positive {
		background: radial-gradient(circle at 38% 32%, #34d36a, #16a34a);
	}
	.disc.negative {
		background: radial-gradient(circle at 38% 32%, #60a5fa, #2563eb);
	}
	.ring {
		width: 55%;
		aspect-ratio: 1;
		border-radius: 50%;
		border: 2px solid rgba(205, 205, 210, 0.5);
	}
	.dot {
		width: 14%;
		aspect-ratio: 1;
		border-radius: 50%;
		background: rgba(150, 150, 160, 0.45);
	}

	@media (prefers-reduced-motion: no-preference) {
		.quad,
		.disc,
		.ring {
			transition: opacity 0.25s ease;
		}
	}
</style>
