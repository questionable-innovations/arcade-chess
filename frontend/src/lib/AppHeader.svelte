<script lang="ts">
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
	<div class="brand">
		<span class="mark"></span>
		<span class="word">ARCADE&nbsp;CHESS</span>
		<span class="sub">bring-up</span>
	</div>
	<div class="spacer"></div>
	<div class="status" class:muted={!device}>
		<span class="dot" class:up={ws.connected}></span>
		{#if device}
			<span class="tnum">{device.device_id}</span>
			<span class="div">/</span>
			<span>{ageLabel(device)}</span>
		{:else}
			<span>{ws.connected ? 'no board' : 'connecting…'}</span>
		{/if}
	</div>
	<div class="admin">
		{#if ws.authed}
			<span class="pill authed"><span class="d"></span>admin</span>
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
			<button class="pill ghost" onclick={() => (adminOpen = true)}>admin</button>
		{/if}
	</div>
	<button
		class="pill toggle"
		class:on={debug}
		onclick={() => (debug = !debug)}
		title="toggle debug (d)"
	>
		<span class="d"></span>debug
	</button>
</header>

<style>
	header {
		display: flex;
		align-items: center;
		gap: 14px;
		padding: 14px 20px;
		border-bottom: 1px solid var(--color-line-soft);
	}
	.brand {
		display: flex;
		align-items: baseline;
		gap: 9px;
	}
	.mark {
		width: 11px;
		height: 11px;
		border-radius: 3px;
		align-self: center;
		background: linear-gradient(135deg, var(--color-pos), var(--color-neg));
		box-shadow: 0 0 0 1px rgba(255, 255, 255, 0.12) inset;
	}
	.word {
		font-weight: 700;
		font-size: 14px;
		letter-spacing: 0.16em;
		color: var(--color-fg);
	}
	.sub {
		font-family: var(--font-mono);
		font-size: 10px;
		letter-spacing: 0.1em;
		color: var(--color-fg-ghost);
		text-transform: uppercase;
	}
	.spacer {
		flex: 1;
	}
	.status {
		display: flex;
		align-items: center;
		gap: 8px;
		font-family: var(--font-mono);
		font-size: 12px;
		color: var(--color-fg-dim);
	}
	.status.muted {
		color: var(--color-fg-faint);
	}
	.status .div {
		color: var(--color-fg-ghost);
	}
	.status .dot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: var(--color-fault);
		box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-fault) 18%, transparent);
	}
	.status .dot.up {
		background: var(--color-live);
		box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-live) 18%, transparent);
	}

	.pill {
		display: inline-flex;
		align-items: center;
		gap: 7px;
		height: 30px;
		padding: 0 13px;
		font-family: var(--font-mono);
		font-size: 11px;
		letter-spacing: 0.06em;
		color: var(--color-fg-dim);
		background: var(--color-surface);
		border: 1px solid var(--color-line);
		border-radius: 999px;
		cursor: pointer;
		transition:
			color 0.15s ease,
			border-color 0.15s ease,
			background 0.15s ease;
	}
	.pill:hover {
		color: var(--color-fg);
		border-color: var(--color-fg-faint);
	}
	.pill .d {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: currentColor;
		opacity: 0.55;
	}
	.pill.ghost {
		color: var(--color-fg-faint);
		background: transparent;
		border-color: transparent;
	}
	.pill.toggle.on {
		color: var(--color-probe);
		border-color: color-mix(in srgb, var(--color-probe) 45%, var(--color-line));
		background: color-mix(in srgb, var(--color-probe) 12%, transparent);
	}
	.pill.authed {
		color: var(--color-live);
		border-color: color-mix(in srgb, var(--color-live) 40%, var(--color-line));
	}
	.admin input {
		height: 30px;
		width: 150px;
		padding: 0 12px;
		font-family: var(--font-mono);
		font-size: 11px;
		color: var(--color-fg);
		background: var(--color-surface);
		border: 1px solid var(--color-probe);
		border-radius: 999px;
		outline: none;
	}
</style>
