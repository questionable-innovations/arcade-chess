<script lang="ts">
	// Sixteen segments, mirroring the hardware edge bar exactly, so screen and
	// board read identically and a mis-mapped strip is obvious rather than
	// merely wrong. Starts at 8/8 — the 50/50 the game opens on.
	//
	// The source badge is not debug output. A material count labelled
	// "stockfish" would be worse than no eval at all.

	import { formatEval, type GameState } from './types';

	interface Props {
		game: GameState;
		vertical?: boolean;
	}
	let { game, vertical = false }: Props = $props();

	const SEGMENTS = 16;
	const fill = $derived(
		Math.min(SEGMENTS - 1, Math.max(1, Math.round(game.eval.win_prob * SEGMENTS)))
	);
	const label = $derived(formatEval(game));
	const swing = $derived(game.eval.cp - game.eval.start_cp);
</script>

<div class="wrap" class:vertical>
	<div class="head">
		<span class="num tnum" class:pending={game.eval.status !== 'ok'}>{label}</span>
		<span
			class="source"
			class:manual={game.eval.source === 'admin'}
			class:soft={game.eval.source === 'material' || game.eval.source === 'unknown'}
		>
			<!-- `unknown` is not a source, it is the absence of one: nothing has
			     been evaluated yet, and saying "material" here would put a badge on
			     a number that was never counted. -->
			{game.eval.source === 'unknown' ? 'no eval yet' : game.eval.source}
		</span>
	</div>
	<div class="bar" role="img" aria-label={`evaluation ${label}, white ${fill} of ${SEGMENTS}`}>
		{#each Array.from({ length: SEGMENTS }, (_, i) => i) as i (i)}
			<span class="seg" class:white={i < fill} class:mid={i === SEGMENTS / 2 - 1}></span>
		{/each}
	</div>
	{#if game.ply > 0}
		<!-- The verdict judges the swing, not the absolute, so the swing is what
		     gets said out loud. -->
		<div class="swing tnum">
			swing {swing >= 0 ? '+' : ''}{(swing / 100).toFixed(2)} from {(
				game.eval.start_cp / 100
			).toFixed(2)}
		</div>
	{/if}
</div>

<style>
	.wrap {
		display: flex;
		flex-direction: column;
		gap: 7px;
		min-width: min(200px, 100%);
	}
	.head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 10px;
	}
	.num {
		font-family: var(--font-mono);
		font-size: 26px;
		font-weight: 600;
		letter-spacing: -0.01em;
	}
	.num.pending {
		color: var(--color-fg-faint);
	}
	.source {
		font-family: var(--font-mono);
		font-size: 10px;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--color-live);
	}
	.source.soft {
		color: var(--color-warn);
	}
	.source.manual {
		color: var(--color-probe);
	}

	.bar {
		display: grid;
		grid-template-columns: repeat(16, 1fr);
		gap: 2px;
		height: 22px;
	}
	.vertical .bar {
		grid-template-columns: 1fr;
		grid-auto-rows: 1fr;
		height: 260px;
	}
	.seg {
		background: #30344f;
		border-radius: 2px;
		transition: background 0.35s ease;
	}
	.seg.white {
		background: #e8e8e8;
	}
	.seg.mid {
		box-shadow: inset 0 0 0 1px rgb(255 255 255 / 0.25);
	}

	.swing {
		font-family: var(--font-mono);
		font-size: 11px;
		color: var(--color-fg-faint);
	}
</style>
