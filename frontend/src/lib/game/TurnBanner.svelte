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

	// The side-to-move token means something only while there is a side to move;
	// beside "Game over" it was just a circle.
	const showTurn = $derived(game.phase === 'playing' || game.phase === 'awaiting_choice');

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
		{#if showTurn}<span class="dot" class:white={game.turn === 'white'}></span>{/if}
		<h1>{headline}</h1>
	</div>
	{#if subline}<p class="sub">{subline}</p>{/if}

	<!-- `degraded` is a product feature, not debug output: it gives the
	     presenter something honest to say when a subsystem drops. -->
	{#if game.degraded.length}
		<p class="degraded">{game.degraded.map(degradedLabel).join(' · ')}</p>
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
	.degraded {
		margin: 2px 0 0;
		font-family: var(--font-mono);
		font-size: 11px;
		letter-spacing: 0.03em;
		color: var(--color-warn);
	}

	/* Landscape on a phone: every line this banner spends is a rank of board
	   pushed off the screen. Still the loudest thing on the page, just smaller. */
	@media (orientation: landscape) and (max-height: 560px) {
		.banner {
			gap: 1px;
		}
		h1 {
			font-size: 21px;
		}
		.dot {
			width: 12px;
			height: 12px;
		}
		.sub {
			font-size: 11px;
		}
		.degraded {
			margin-top: 0;
			font-size: 10px;
		}
	}
</style>
