<script lang="ts">
	// Arcade puzzle mode. `/` is untouched: it stays one tab away as the raw
	// lighting / snapshot / calibration / bus-trace diagnostics tool, which is
	// exactly what you want reachable when the electronics act up mid-demo.
	//
	// Two simultaneous consumers: the projector (unauthenticated, readable
	// across a room) and the operator's phone (authed, admin rail).

	import { resolve } from '$app/paths';
	import AdminRail from '$lib/game/AdminRail.svelte';
	import ChoicePrompt from '$lib/game/ChoicePrompt.svelte';
	import EvalBar from '$lib/game/EvalBar.svelte';
	import GameBoard from '$lib/game/GameBoard.svelte';
	import ResultOverlay from '$lib/game/ResultOverlay.svelte';
	import SetupPanel from '$lib/game/SetupPanel.svelte';
	import TurnBanner from '$lib/game/TurnBanner.svelte';
	import { squareName } from '$lib/game/types';
	import { ws } from '$lib/ws.svelte';

	const game = $derived(ws.game);
	const canControl = $derived(ws.authed);
	let maskArmed = $state(false);
	let resultDismissed = $state(false);

	$effect(() => {
		ws.connect();
		return () => ws.teardown();
	});

	// A fresh result is a fresh splash: dismissing one game's overlay must not
	// silently swallow the next.
	let shownSeq = $state(-1);
	$effect(() => {
		if (game.phase === 'finished' && game.game_seq !== shownSeq) {
			shownSeq = game.game_seq;
			resultDismissed = false;
		}
	});

	function send(action: string, extra: Record<string, unknown> = {}) {
		ws.sendGame(action, extra);
	}

	function onMaskSquare(square: number) {
		send('mask_square', { square, masked: !game.detect.masked.includes(square) });
		maskArmed = false;
	}

	const showSetup = $derived(game.phase === 'setup' || game.phase === 'countdown');
	const nudge = $derived(game.detect.nudge);
</script>

<svelte:head><title>Arcade Chess — puzzle</title></svelte:head>

<div class="page">
	<header class="top">
		<a class="back" href={resolve('/')}>← diagnostics</a>
		<TurnBanner {game} />
		<span class="link" class:live={ws.connected}>{ws.connected ? 'connected' : 'offline'}</span>
	</header>

	<main class="stage">
		<div class="left">
			<EvalBar {game} />

			{#if game.phase === 'idle'}
				<section class="card intro">
					<h3>Arcade puzzle</h3>
					<p>
						A position that is dead level and one move from decided. Build it on the board, five
						moves each, and the engine says who gained ground.
					</p>
					<button class="start" disabled={!canControl} onclick={() => send('new_game')}>
						New game
					</button>
					{#if !canControl}<p class="note">sign in on the right to deal</p>{/if}
				</section>
			{/if}

			{#if showSetup}
				<SetupPanel {game} {canControl} onStart={() => send('start')} />
			{/if}

			{#if game.moves.length}
				<section class="card moves">
					<h3>Moves</h3>
					<ol>
						{#each game.moves as move, i (i)}
							<li>
								<span class="n tnum">{Math.floor(i / 2) + 1}{i % 2 === 0 ? '.' : '…'}</span>
								<span class="san">{move.san}</span>
								{#if move.by !== 'sensor'}<span class="by">{move.by}</span>{/if}
								{#if move.confidence === 'likely'}<span class="by soft">likely</span>{/if}
							</li>
						{/each}
					</ol>
				</section>
			{/if}
		</div>

		<div class="middle">
			<GameBoard
				{game}
				interactive={canControl && (game.phase === 'playing' || game.phase === 'awaiting_choice')}
				{maskArmed}
				onMove={(uci) => send('move', { uci })}
				{onMaskSquare}
			/>

			<!-- Captured pieces parked on an empty square read as an unexplained
			     piece and downgrade the ply to a prompt, so the instruction stays
			     on screen the whole game. -->
			{#if game.phase === 'playing' || game.phase === 'awaiting_choice'}
				<p class="rule">Captured pieces go <strong>off the board</strong></p>
			{/if}

			{#if nudge}
				<p class="nudge">
					Nudge the piece from {squareName(nudge.actual)} onto
					<strong>{squareName(nudge.expected)}</strong>
				</p>
			{/if}

			<ChoicePrompt {game} {canControl} onChoose={(uci) => send('choose', { uci })} />
		</div>

		<AdminRail
			{game}
			authed={ws.authed}
			deviceIds={ws.order}
			{maskArmed}
			{send}
			onArmMask={(armed) => (maskArmed = armed)}
			onAuth={(password) => ws.auth(password)}
		/>
	</main>
</div>

{#if !resultDismissed}
	<ResultOverlay
		{game}
		{canControl}
		onNewGame={() => {
			resultDismissed = true;
			send('new_game');
		}}
		onDismiss={() => (resultDismissed = true)}
	/>
{/if}

<style>
	.page {
		min-height: 100vh;
		display: grid;
		grid-template-rows: auto 1fr;
		position: relative;
		z-index: 1;
	}

	.top {
		display: grid;
		grid-template-columns: 120px 1fr 120px;
		align-items: center;
		padding: 16px clamp(14px, 3vw, 32px) 4px;
	}
	.back,
	.link {
		font-family: var(--font-mono);
		font-size: 11px;
		color: var(--color-fg-faint);
		text-decoration: none;
	}
	.link {
		text-align: right;
	}
	.link.live {
		color: var(--color-live);
	}
	.back:hover {
		color: var(--color-fg);
	}

	.stage {
		display: grid;
		grid-template-columns: 300px minmax(0, 1fr) auto;
		gap: clamp(14px, 2.5vw, 32px);
		align-items: start;
		padding: clamp(12px, 2.5vw, 30px);
	}
	.left {
		display: flex;
		flex-direction: column;
		gap: 14px;
	}
	.middle {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 14px;
	}

	.intro p {
		margin: 0 0 12px;
		font-size: 12.5px;
		line-height: 1.5;
		color: var(--color-fg-dim);
	}
	.start {
		width: 100%;
		padding: 12px;
		font-family: inherit;
		font-size: 15px;
		font-weight: 600;
		color: var(--color-ink);
		background: var(--color-live);
		border: 0;
		border-radius: 9px;
		cursor: pointer;
	}
	.start:disabled {
		opacity: 0.35;
		cursor: not-allowed;
	}

	.rule {
		margin: 0;
		font-family: var(--font-mono);
		font-size: 11px;
		letter-spacing: 0.05em;
		color: var(--color-fg-faint);
	}
	.rule strong {
		color: var(--color-warn);
	}
	.nudge {
		margin: 0;
		padding: 9px 16px;
		font-size: 15px;
		color: var(--color-ink);
		background: var(--color-warn);
		border-radius: 8px;
	}

	.moves ol {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 1px 12px;
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.moves li {
		display: flex;
		align-items: baseline;
		gap: 6px;
		font-size: 12.5px;
	}
	.n {
		font-family: var(--font-mono);
		font-size: 10px;
		color: var(--color-fg-faint);
		min-width: 24px;
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

	@media (max-width: 1180px) {
		.stage {
			grid-template-columns: minmax(0, 1fr);
		}
		.middle {
			order: -1;
		}
	}
</style>
