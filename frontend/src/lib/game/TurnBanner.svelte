<script lang="ts">
	// The phase banner and the turn indicator are the same thing: at any moment
	// there is exactly one sentence the room needs, and this is where it goes.
	// Readable across a room is the requirement, not decoration.

	import { degradedLabel, type GameState } from './types';

	interface Props {
		game: GameState;
	}
	let { game }: Props = $props();

	const moveNumber = $derived(Math.floor(game.ply / 2) + 1);
	const totalMoves = $derived(Math.ceil(game.max_ply / 2));

	const headline = $derived.by(() => {
		switch (game.phase) {
			case 'idle':
				return 'Ready';
			case 'setup':
				return 'Build the position';
			case 'countdown':
				return 'Starting';
			case 'scoring':
				return 'Scoring';
			case 'finished':
				return 'Game over';
			case 'awaiting_choice':
				return 'Confirm';
			default:
				return game.turn === 'white' ? 'White to move' : 'Black to move';
		}
	});

	const subline = $derived.by(() => {
		if (game.phase === 'setup') {
			const s = game.setup;
			return s ? `${s.placed} of ${s.needed} placed` : '';
		}
		if (game.phase === 'countdown') {
			const ms = game.setup?.auto_start_in_ms ?? 0;
			return `${Math.max(1, Math.ceil(ms / 1000))}…`;
		}
		if (game.phase === 'idle') return 'press New game';
		if (game.phase === 'finished') return '';
		return `Move ${Math.min(moveNumber, totalMoves)} of ${totalMoves}`;
	});
</script>

<div class="banner" class:black={game.turn === 'black' && game.phase === 'playing'}>
	<div class="line">
		<span class="dot" class:white={game.turn === 'white'}></span>
		<h1>{headline}</h1>
	</div>
	{#if subline}<p class="sub">{subline}</p>{/if}

	<!-- `degraded` is a product feature, not debug output: it gives the
	     presenter something honest to say when a subsystem drops. -->
	{#if game.degraded.length}
		<div class="chips">
			{#each game.degraded as code (code)}
				<span class="chip">{degradedLabel(code)}</span>
			{/each}
		</div>
	{/if}
</div>

<style>
	.banner {
		display: flex;
		flex-direction: column;
		gap: 5px;
		align-items: center;
		text-align: center;
	}
	.line {
		display: flex;
		align-items: center;
		gap: 12px;
	}
	h1 {
		margin: 0;
		font-size: clamp(22px, 3.4vw, 38px);
		font-weight: 600;
		letter-spacing: -0.02em;
	}
	.dot {
		width: 16px;
		height: 16px;
		border-radius: 50%;
		background: #14161a;
		border: 2px solid var(--color-fg-faint);
		flex: none;
	}
	.dot.white {
		background: #f2f4f6;
		border-color: #f2f4f6;
	}
	.sub {
		margin: 0;
		font-family: var(--font-mono);
		font-size: 13px;
		color: var(--color-fg-dim);
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		justify-content: center;
		margin-top: 4px;
	}
	.chip {
		font-family: var(--font-mono);
		font-size: 10px;
		letter-spacing: 0.03em;
		color: var(--color-warn);
		background: color-mix(in srgb, var(--color-warn) 10%, transparent);
		border: 1px solid color-mix(in srgb, var(--color-warn) 40%, var(--color-line));
		border-radius: 999px;
		padding: 2px 9px;
	}
</style>
