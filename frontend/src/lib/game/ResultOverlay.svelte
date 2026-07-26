<script lang="ts">
	// The full-screen winner splash. The number it shows is the **swing**, not
	// the absolute eval: the mining band is ±40 cp, so a dealt position can sit
	// at +35 with nobody having done anything, and "White won" with no way to
	// explain it is worse than no verdict at all.

	import type { GameState } from './types';

	interface Props {
		game: GameState;
		canControl: boolean;
		onNewGame: () => void;
		onDismiss: () => void;
	}
	let { game, canControl, onNewGame, onDismiss }: Props = $props();

	const result = $derived(game.result);
	const title = $derived.by(() => {
		if (!result) return '';
		if (result.winner === 'draw') return 'Level';
		return result.winner === 'white' ? 'White takes it' : 'Black takes it';
	});
	const reason = $derived.by(() => {
		if (!result) return '';
		switch (result.reason) {
			case 'mate':
				return 'checkmate';
			case 'stalemate':
				return 'stalemate';
			case 'admin':
				return 'called by the operator';
			default:
				return `judged on the swing over ${game.ply} plies`;
		}
	});
	const swing = $derived(((result?.swing ?? 0) / 100).toFixed(2));
</script>

{#if result}
	<div class="overlay">
		<div class="panel">
			<p class="kicker">{reason}</p>
			<h2 class:white={result.winner === 'white'} class:black={result.winner === 'black'}>
				{title}
			</h2>
			<p class="swing tnum">
				{result.swing >= 0 ? '+' : ''}{swing}
				<span class="unit">pawns of ground gained</span>
			</p>

			<ol class="moves">
				{#each game.moves as move, i (i)}
					<li>
						<span class="n tnum">{Math.floor(i / 2) + 1}{i % 2 === 0 ? '.' : '…'}</span>
						<span class="san">{move.san}</span>
						{#if move.by !== 'sensor'}<span class="by">{move.by}</span>{/if}
						{#if move.confidence === 'likely'}<span class="by soft">likely</span>{/if}
					</li>
				{/each}
			</ol>

			<div class="actions">
				<button class="primary" disabled={!canControl} onclick={onNewGame}>New game</button>
				<button class="ghost" onclick={onDismiss}>Look at the board</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.overlay {
		position: fixed;
		inset: 0;
		z-index: 20;
		display: grid;
		place-items: center;
		padding: 24px;
		background: rgb(6 7 8 / 0.86);
		backdrop-filter: blur(8px);
	}
	.panel {
		width: min(560px, 100%);
		max-height: 90vh;
		overflow-y: auto;
		display: flex;
		flex-direction: column;
		gap: 10px;
		align-items: center;
		text-align: center;
		padding: 30px 28px;
		background: var(--color-surface);
		border: 1px solid var(--color-line);
		border-radius: 16px;
	}
	.kicker {
		margin: 0;
		font-family: var(--font-mono);
		font-size: 11px;
		letter-spacing: 0.09em;
		text-transform: uppercase;
		color: var(--color-fg-faint);
	}
	h2 {
		margin: 0;
		font-size: clamp(30px, 6vw, 52px);
		font-weight: 700;
		letter-spacing: -0.03em;
	}
	h2.white {
		color: #f2f4f6;
	}
	h2.black {
		color: #9aa6d6;
	}
	.swing {
		margin: 0;
		font-family: var(--font-mono);
		font-size: 22px;
		color: var(--color-live);
	}
	.unit {
		font-size: 11px;
		color: var(--color-fg-faint);
	}

	.moves {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 2px 16px;
		margin: 10px 0 4px;
		padding: 0;
		list-style: none;
		width: 100%;
	}
	.moves li {
		display: flex;
		align-items: baseline;
		gap: 7px;
		font-size: 13px;
	}
	.n {
		font-family: var(--font-mono);
		font-size: 11px;
		color: var(--color-fg-faint);
		min-width: 26px;
		text-align: right;
	}
	.san {
		font-weight: 600;
	}
	.by {
		font-family: var(--font-mono);
		font-size: 9px;
		color: var(--color-probe);
	}
	.by.soft {
		color: var(--color-warn);
	}

	.actions {
		display: flex;
		gap: 10px;
		margin-top: 12px;
	}
	button {
		padding: 11px 22px;
		font-family: inherit;
		font-size: 14px;
		font-weight: 600;
		border-radius: 9px;
		cursor: pointer;
	}
	.primary {
		color: var(--color-ink);
		background: var(--color-live);
		border: 0;
	}
	.primary:disabled {
		opacity: 0.35;
		cursor: not-allowed;
	}
	.ghost {
		color: var(--color-fg-dim);
		background: transparent;
		border: 1px solid var(--color-line);
	}
</style>
