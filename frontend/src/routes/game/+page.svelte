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
	import { isProjector } from '$lib/backend';

	const game = $derived(ws.game);
	// The projector is a second, unauthenticated consumer of the same page.
	// Resolved after mount rather than at init: the prerender pass has no
	// `location`, and the query string is a client-side fact anyway.
	let projector = $state(false);
	const canControl = $derived(ws.authed && !projector);
	let maskArmed = $state(false);
	let resultDismissed = $state(false);

	$effect(() => {
		projector = isProjector();
		ws.connect();
		return () => ws.teardown();
	});

	// A fresh result is a fresh splash: dismissing one game's overlay must not
	// silently swallow the next.
	//
	// Keyed on the *result*, not on `game_seq`: an admin decreeing the winner
	// calls `finish()` without bumping the sequence, so a dismiss-then-decree
	// left the corrected verdict with nowhere on screen to appear and no way
	// short of a page reload to get it back.
	let shownResult = $state('');
	$effect(() => {
		const key = game.result ? JSON.stringify(game.result) : '';
		if (key && key !== shownResult) {
			shownResult = key;
			resultDismissed = false;
		}
	});

	// A frozen board that looks live is what makes transient faults unreadable.
	// `degraded` cannot cover this — it is computed server-side, so a client that
	// cannot reach the server can never be told by it. Same derivation the
	// bring-up dashboard already uses.
	const linkDown = $derived.by((): string | null => {
		if (!ws.connected) return 'link down — showing the last known position';
		if (game.device_id && !game.detect.sensors_live) return 'board offline — screen only';
		return null;
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

<div class="page" class:projector>
	<header class="top">
		{#if !projector}<a class="back" href={resolve('/')}>← diagnostics</a>{/if}
		<div class="banner-slot"><TurnBanner {game} /></div>
		<div class="head-right">
			{#if game.result && resultDismissed}
				<!-- The result lives in exactly one component, so once the splash is
				     dismissed there is otherwise no way back to it. -->
				<button class="chip recall" onclick={() => (resultDismissed = false)}>show result</button>
			{/if}
			<span class="link" class:live={ws.connected}>{ws.connected ? 'connected' : 'offline'}</span>
		</div>
	</header>

	{#if linkDown}
		<!-- Rendered in the same amber lane the presenter is already reading, and
		     unmissable across a room: the worst failure mode is the one where the
		     projector confidently narrates a position that no longer exists. -->
		<div class="linkdown" role="status">{linkDown}</div>
	{/if}

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
	/* A column, not `grid-template-rows: auto 1fr` — the link-down banner is a
	   conditional third child, and with a fixed two-row template it landed in
	   the `1fr` row and swallowed the page's leftover height. Worst exactly
	   where it matters most: the projector hides the rail, so there is more
	   slack there than anywhere, and the one warning the room must read became
	   a 300px empty amber block. Here the stage takes the slack whether the
	   banner is present or not. */
	.page {
		min-height: 100vh;
		min-height: 100dvh;
		display: flex;
		flex-direction: column;
		position: relative;
		z-index: 1;
	}

	.top {
		display: grid;
		grid-template-columns: 120px minmax(0, 1fr) 120px;
		grid-template-areas: 'back banner link';
		align-items: center;
		gap: 8px;
		padding: 16px clamp(14px, 3vw, 32px) 4px;
		padding-right: calc(clamp(14px, 3vw, 32px) + var(--safe-r));
		padding-left: calc(clamp(14px, 3vw, 32px) + var(--safe-l));
	}
	.back {
		grid-area: back;
	}
	.banner-slot {
		grid-area: banner;
		min-width: 0;
	}
	.back,
	.link {
		font-family: var(--font-mono);
		font-size: 11px;
		color: var(--color-fg-faint);
		text-decoration: none;
	}
	.head-right {
		grid-area: link;
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 8px;
	}
	.link {
		text-align: right;
	}
	.link.live {
		color: var(--color-live);
	}
	.chip.recall {
		font-family: var(--font-mono);
		font-size: 11px;
		padding: 3px 8px;
		border-radius: 999px;
		border: 1px solid var(--color-line);
		background: transparent;
		color: var(--color-fg-dim);
		cursor: pointer;
	}
	/* Loud on purpose. This is the one state the audience must not be allowed to
	   mistake for a working board, and it has to read from the back of a room. */
	/* Everything scales off one font-size step, so the layout is unchanged and
	   only its legibility moves. */
	.page.projector {
		font-size: 1.35rem;
	}
	.page.projector :global(.rail),
	.page.projector .back {
		display: none;
	}
	.page.projector .linkdown {
		font-size: clamp(18px, 2.4vw, 30px);
	}
	/* A capsule the width of its own sentence, not a slab the width of the page —
	   the same badge language the board already uses for STALE. A live pulse does
	   the shouting, so the fill does not have to. */
	.linkdown {
		align-self: center;
		display: inline-flex;
		align-items: center;
		gap: 0.7em;
		/* Never wider than the header's gutter allows, notch included. */
		max-width: calc(100% - 2 * (clamp(14px, 3vw, 32px) + var(--safe-l)));
		margin: 2px 0 10px;
		padding: 0.34em 0.9em;
		border-radius: 999px;
		border: 1px solid color-mix(in srgb, var(--color-warn) 42%, transparent);
		background: color-mix(in srgb, var(--color-warn) 9%, transparent);
		color: var(--color-warn);
		font-family: var(--font-mono);
		font-size: clamp(12px, 1.3vw, 15px);
		letter-spacing: 0.02em;
		text-align: center;
	}
	.linkdown::before {
		content: '';
		flex: none;
		width: 0.5em;
		height: 0.5em;
		border-radius: 50%;
		background: var(--color-warn);
		animation: linkpulse 1.4s ease-in-out infinite;
	}
	@keyframes linkpulse {
		50% {
			opacity: 0.2;
		}
	}
	/* Narrow enough that the sentence wraps, and a stadium round two lines tall
	   reads as a speech bubble. Square it up and pin the dot to the first line. */
	@media (max-width: 460px) {
		.linkdown {
			align-items: flex-start;
			border-radius: 12px;
		}
		.linkdown::before {
			margin-top: 0.42em;
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.linkdown::before {
			animation: none;
		}
	}
	@media (hover: hover) {
		.back:hover {
			color: var(--color-fg);
		}
	}
	/* The only way off this page, and it was a 17px-tall line of text. */
	@media (pointer: coarse) {
		.back {
			display: inline-flex;
			align-items: center;
			min-height: 34px;
			padding-right: 8px;
		}
	}

	.stage {
		/* Grow into the page's leftover height, never shrink below content. */
		flex: 1 0 auto;
		display: grid;
		grid-template-columns: 300px minmax(0, 1fr) auto;
		gap: clamp(14px, 2.5vw, 32px);
		align-items: start;
		padding: clamp(12px, 2.5vw, 30px);
		padding-right: calc(clamp(12px, 2.5vw, 30px) + var(--safe-r));
		padding-bottom: calc(clamp(12px, 2.5vw, 30px) + var(--safe-b));
		padding-left: calc(clamp(12px, 2.5vw, 30px) + var(--safe-l));
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
		min-height: 48px;
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

	/* One column, board first. Below this width the rail stops being a rail and
	   becomes the last section of a page you scroll — which is what an operator
	   holding a phone is doing anyway. */
	@media (max-width: 1180px) {
		.stage {
			grid-template-columns: minmax(0, 1fr);
		}
		.middle {
			order: -1;
		}
	}

	/* Phone. The turn banner is the one sentence the room needs, so it keeps a
	   full row of its own rather than being squeezed between two links. */
	@media (max-width: 700px) {
		.top {
			grid-template-columns: auto auto;
			grid-template-areas:
				'back link'
				'banner banner';
			gap: 10px;
			padding-top: 12px;
		}
		.head-right {
			justify-self: end;
		}
		.stage {
			gap: 16px;
		}
		.rule,
		.nudge {
			text-align: center;
		}
		.nudge {
			font-size: 14px;
		}
		.moves ol {
			gap: 2px 10px;
		}
	}

	/* Phone held sideways. Everything above the board gets out of its way. */
	@media (orientation: landscape) and (max-height: 560px) {
		.top {
			gap: 2px 8px;
			padding-top: 8px;
			padding-bottom: 0;
		}
		.linkdown {
			margin: 0 0 6px;
			font-size: 12px;
		}
		.stage {
			gap: 10px;
			padding-top: 8px;
		}
		.middle {
			gap: 8px;
		}
	}

	/* Two move columns stop being two columns' worth of width. */
	@media (max-width: 380px) {
		.moves ol {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
