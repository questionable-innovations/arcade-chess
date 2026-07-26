<script lang="ts">
	import { resolve } from '$app/paths';
	import type { DeviceState } from './types';
	import { ws } from './ws.svelte';

	let {
		device,
		now,
		debug = $bindable()
	}: {
		device: DeviceState | null;
		/** Wall clock the relative age readout is measured against. */
		now: number;
		debug: boolean;
	} = $props();

	let adminOpen = $state(false);
	let password = $state('');

	function submitAuth(e: Event) {
		e.preventDefault();
		if (password) ws.auth(password);
		adminOpen = false;
	}

	function ageLabel(dev: DeviceState): string {
		if (!dev.lastEventAt) return '—';
		const s = Math.max(0, Math.round((now - dev.lastEventAt) / 1000));
		return s < 1 ? 'live' : `${s}s ago`;
	}
</script>

<header>
	<span class="word">ARCADE&nbsp;CHESS</span>
	<div class="spacer"></div>

	<!-- Status and the two toggles travel together: on a phone they drop to a
	     second line as one group, so the wordmark and the way out of here stay
	     on the line a thumb reaches first. -->
	<div class="tools">
		<!-- The link used to be a coloured dot beside a word. The word was always
		     the part that got read, so it is now the whole of it. -->
		<div class="status">
			{#if !ws.connected}
				<span class="bad">link down</span>
			{:else if device}
				<span class="tnum">{device.device_id}</span>
				<span class="div">/</span>
				<span>{ageLabel(device)}</span>
			{:else}
				<span>no board</span>
			{/if}
		</div>

		{#if ws.authed}
			<span class="act live">admin</span>
		{:else if adminOpen}
			<form onsubmit={submitAuth}>
				<!-- svelte-ignore a11y_autofocus -->
				<input
					type="password"
					bind:value={password}
					placeholder="password"
					autocomplete="current-password"
					autofocus
					onblur={() => (adminOpen = false)}
				/>
			</form>
		{:else}
			<button class="act" onclick={() => (adminOpen = true)}>admin</button>
		{/if}

		<button class="act" class:on={debug} onclick={() => (debug = !debug)} title="toggle debug (d)">
			debug
		</button>
	</div>

	<!-- The game is what this machine is for; the dashboard is the tool behind
	     it. One press away, from every screen. -->
	<a class="play" href={resolve('/game')} title="game mode (g)">game mode →</a>
</header>

<style>
	header {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 10px 16px;
		padding: 14px calc(20px + var(--safe-r)) 14px calc(20px + var(--safe-l));
		border-bottom: 1px solid var(--color-line-soft);
	}
	.word {
		font-weight: 700;
		font-size: 14px;
		letter-spacing: 0.16em;
		color: var(--color-fg);
	}
	.spacer {
		flex: 1;
	}
	.tools {
		display: flex;
		align-items: center;
		gap: 16px;
		min-width: 0;
	}
	.status {
		display: flex;
		align-items: center;
		gap: 7px;
		font-family: var(--font-mono);
		font-size: 12px;
		color: var(--color-fg-dim);
	}
	.status .div {
		color: var(--color-fg-ghost);
	}
	.status .bad {
		color: var(--color-fault);
	}

	.act {
		font-family: var(--font-mono);
		font-size: 11px;
		letter-spacing: 0.06em;
		color: var(--color-fg-faint);
		background: transparent;
		border: 0;
		padding: 4px 0;
		cursor: pointer;
		transition: color 0.15s ease;
	}
	@media (hover: hover) {
		.act:hover {
			color: var(--color-fg);
		}
	}
	.act.on {
		color: var(--color-probe);
	}
	.act.live {
		color: var(--color-live);
		cursor: default;
	}
	@media (pointer: coarse) {
		.act {
			padding: 9px 0;
		}
	}

	.play {
		display: inline-flex;
		align-items: center;
		height: 28px;
		padding: 0 12px;
		font-family: var(--font-mono);
		font-size: 11px;
		letter-spacing: 0.06em;
		color: var(--color-live);
		text-decoration: none;
		background: transparent;
		border: 1px solid color-mix(in srgb, var(--color-live) 38%, var(--color-line));
		border-radius: 6px;
		transition:
			background 0.15s ease,
			border-color 0.15s ease;
	}
	@media (hover: hover) {
		.play:hover {
			background: color-mix(in srgb, var(--color-live) 12%, transparent);
			border-color: color-mix(in srgb, var(--color-live) 60%, var(--color-line));
		}
	}

	form {
		min-width: 0;
	}
	form input {
		height: 28px;
		width: 150px;
		max-width: 100%;
		padding: 0 10px;
		font-family: var(--font-mono);
		font-size: 11px;
		color: var(--color-fg);
		background: var(--color-surface);
		border: 1px solid var(--color-probe);
		border-radius: 6px;
		outline: none;
	}

	/* Phone: wordmark and the way out on top, instruments underneath. The
	   `order` is what keeps `game mode` on the first line once `.tools` claims
	   a whole one for itself. */
	@media (max-width: 640px) {
		header {
			gap: 12px;
			padding: 12px calc(14px + var(--safe-r)) 12px calc(14px + var(--safe-l));
		}
		.word {
			order: 1;
			font-size: 12.5px;
		}
		.spacer {
			display: none;
		}
		.play {
			order: 2;
			margin-left: auto;
		}
		.tools {
			order: 3;
			flex-basis: 100%;
			flex-wrap: wrap;
			justify-content: space-between;
			gap: 10px 12px;
		}
		.status {
			flex: 1 1 auto;
			min-width: 0;
			overflow: hidden;
			text-overflow: ellipsis;
			white-space: nowrap;
		}
		.act {
			padding: 8px 0;
		}
		/* The password field earns the whole row rather than sharing one. */
		.tools form {
			order: 4;
			flex: 1 1 100%;
		}
		form input {
			height: 36px;
			width: 100%;
		}
	}
</style>
