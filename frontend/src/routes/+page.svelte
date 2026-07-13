<script lang="ts">
	import Board from '$lib/Board.svelte';
	import { ws } from '$lib/ws.svelte';
	import { type DeviceState, type Envelope, type SquareState } from '$lib/types';

	const emptySquares: SquareState[] = Array.from({ length: 64 }, () => 'empty');
	const emptyValid: boolean[] = Array.from({ length: 64 }, () => true);
	const emptyNodes: (Envelope | null)[] = [null, null, null, null];

	let now = $state(Date.now());
	let adminOpen = $state(false);
	let password = $state('');

	$effect(() => {
		ws.connect();
		const t = setInterval(() => (now = Date.now()), 1000);
		return () => clearInterval(t);
	});

	const selected = $derived.by((): DeviceState | null => {
		const ids = ws.order;
		if (!ids.length) return null;
		const online = ids.find((id) => ws.devices[id]?.connected);
		return ws.devices[online ?? ids[0]] ?? null;
	});

	function submitAuth(e: Event) {
		e.preventDefault();
		if (password) ws.auth(password);
	}

	function onSquare(i: number) {
		if (ws.authed && selected) ws.probe(selected.device_id, i);
	}

	function ageLabel(dev: DeviceState): string {
		if (!dev.lastEventAt) return '—';
		const s = Math.max(0, Math.round((now - dev.lastEventAt) / 1000));
		return `${s}s ago`;
	}
</script>

<svelte:head><title>chess</title></svelte:head>

<main>
	{#if selected}
		<Board
			squares={selected.squares}
			valid={selected.valid}
			nodeStatus={selected.node_status}
			admin={ws.authed}
			{onSquare}
		/>
		<div class="status">
			<span>{selected.device_id}</span>
			<span class="sep">·</span>
			<span>{selected.bootId ?? '—'}</span>
			<span class="sep">·</span>
			<span>seq {selected.seq < 0 ? '—' : selected.seq}{selected.gap ? ' !gap' : ''}</span>
			<span class="sep">·</span>
			<span>{ageLabel(selected)}</span>
			<span class="sep">·</span>
			<span class="wsdot" class:up={ws.connected}></span>
		</div>
	{:else}
		<Board
			squares={emptySquares}
			valid={emptyValid}
			nodeStatus={emptyNodes}
			admin={false}
			onSquare={() => {}}
		/>
		<div class="status muted">no board</div>
	{/if}

	<div class="ticker">
		{#each ws.events as ev (ev.id)}
			<div class="tick"><span class="at">{ev.at}</span> {ev.text}</div>
		{/each}
	</div>

	<div class="admin">
		{#if ws.authed}
			<span class="authed">admin</span>
		{:else if adminOpen}
			<form onsubmit={submitAuth}>
				<!-- svelte-ignore a11y_autofocus -->
				<input
					type="password"
					bind:value={password}
					placeholder="password"
					autocomplete="off"
					autofocus
				/>
			</form>
		{:else}
			<button class="adminbtn" onclick={() => (adminOpen = true)}>admin</button>
		{/if}
	</div>
</main>

<style>
	main {
		min-height: 100vh;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 14px;
		padding: 24px;
	}

	.status {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: 11px;
		color: #6b6b72;
	}
	.status .sep {
		color: #3a3a40;
	}
	.status.muted {
		color: #4a4a50;
	}
	.wsdot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: #ef4444;
	}
	.wsdot.up {
		background: #22c55e;
	}

	.ticker {
		width: min(72vh, 90vw);
		display: flex;
		flex-direction: column;
		gap: 2px;
		font-size: 11px;
		color: #55555c;
	}
	.tick {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.tick .at {
		color: #3f3f45;
	}

	.admin {
		position: fixed;
		right: 10px;
		bottom: 8px;
		font-size: 11px;
	}
	.adminbtn {
		border: 0;
		background: none;
		padding: 0;
		font: inherit;
		color: #2f2f34;
		cursor: pointer;
	}
	.adminbtn:hover {
		color: #6b6b72;
	}
	.authed {
		color: #22c55e;
	}
	.admin input {
		background: #111113;
		border: 1px solid #2a2a2e;
		color: #c7c7cc;
		font: inherit;
		font-size: 11px;
		padding: 2px 6px;
		border-radius: 3px;
		outline: none;
		width: 120px;
	}
</style>
