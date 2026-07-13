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
	let showLog = $state(false);
	let showRaw = $state(false);

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

	const rawScan = $derived(selected?.raw_scan ?? null);

	// Latest raw ADC counts as display rows, rank 8 on top to match the board.
	const rawRows = $derived.by((): (number | null)[][] | null => {
		const adc = rawScan?.data?.raw_adc;
		if (!adc) return null;
		const rows: (number | null)[][] = [];
		for (let r = 7; r >= 0; r--) {
			const row: (number | null)[] = [];
			for (let c = 0; c < 8; c++) row.push(adc[r * 8 + c] ?? null);
			rows.push(row);
		}
		return rows;
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
		return `${s}s`;
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
			<span class="wsdot" class:up={ws.connected}></span>
			<span>{selected.device_id}</span>
			<span class="sep">·</span>
			<span>{ageLabel(selected)}</span>
			<button class="logbtn" class:open={showLog} onclick={() => (showLog = !showLog)}>
				{showLog ? '× log' : '▸ log'}
			</button>
			<button class="logbtn" class:open={showRaw} onclick={() => (showRaw = !showRaw)}>
				{showRaw ? '× raw' : '▸ raw'}
			</button>
		</div>
	{:else}
		<Board
			squares={emptySquares}
			valid={emptyValid}
			nodeStatus={emptyNodes}
			admin={false}
			onSquare={() => {}}
		/>
		<div class="status muted">
			<span class="wsdot" class:up={ws.connected}></span>
			<span>no board</span>
		</div>
	{/if}
</main>

{#if showRaw}
	<div class="raw">
		{#if rawScan && rawRows}
			<div class="rawmeta">
				scan {rawScan.data?.scan_id ?? '—'} · {rawScan.data?.complete ? 'complete' : 'partial'} · mask
				{rawScan.data?.response_node_mask ?? '—'}
			</div>
			<div class="rawgrid">
				{#each rawRows as row, r (r)}
					{#each row as v, c (c)}
						<span
							class="rawcell"
							style:background={v == null
								? undefined
								: `rgba(0, 160, 255, ${((v / 1023) * 0.55).toFixed(3)})`}>{v ?? '·'}</span
						>
					{/each}
				{/each}
			</div>
		{:else}
			<div class="rawmeta">no raw scan yet</div>
		{/if}
	</div>
{/if}

{#if showLog}
	<div class="log">
		{#if selected}
			<div class="logmeta">
				boot {selected.bootId ?? '—'} · seq {selected.seq < 0 ? '—' : selected.seq}{selected.gap
					? ' !gap'
					: ''}
			</div>
		{/if}
		{#each ws.events as ev (ev.id)}
			<div class="tick"><span class="at">{ev.at}</span> {ev.text}</div>
		{/each}
	</div>
{/if}

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

<style>
	main {
		min-height: 100vh;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 16px;
		padding: 24px;
	}

	.status {
		display: flex;
		align-items: center;
		gap: 7px;
		font-size: 11px;
		color: #9a9aa4;
	}
	.status .sep {
		color: #4a4a52;
	}
	.status.muted {
		color: #5a5a62;
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
	.logbtn {
		border: 0;
		background: none;
		padding: 0 0 0 6px;
		font: inherit;
		font-size: 11px;
		color: #5a5a62;
		cursor: pointer;
	}
	.logbtn:hover,
	.logbtn.open {
		color: #9a9aa4;
	}

	/* Fixed overlay so opening raw diagnostics never shifts the board. */
	.raw {
		position: fixed;
		right: 12px;
		top: 10px;
		font-size: 11px;
		color: #82828c;
	}
	.rawmeta {
		color: #9a9aa4;
		margin-bottom: 4px;
	}
	.rawgrid {
		display: grid;
		grid-template-columns: repeat(8, 3.2em);
		gap: 1px;
	}
	.rawcell {
		padding: 2px 0;
		text-align: center;
		font-variant-numeric: tabular-nums;
		background: #111113;
	}

	/* Fixed overlay so expanding the log never shifts the board. */
	.log {
		position: fixed;
		left: 12px;
		bottom: 10px;
		max-width: min(46ch, 60vw);
		display: flex;
		flex-direction: column;
		gap: 2px;
		font-size: 11px;
		color: #82828c;
	}
	.logmeta {
		color: #9a9aa4;
		margin-bottom: 2px;
	}
	.tick {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.tick .at {
		color: #55555e;
	}

	.admin {
		position: fixed;
		right: 12px;
		bottom: 10px;
		font-size: 11px;
	}
	.adminbtn {
		border: 0;
		background: none;
		padding: 0;
		font: inherit;
		color: #3a3a42;
		cursor: pointer;
	}
	.adminbtn:hover {
		color: #9a9aa4;
	}
	.authed {
		color: #22c55e;
	}
	.admin input {
		background: #111113;
		border: 1px solid #2e2e36;
		color: #d2d2da;
		font: inherit;
		font-size: 11px;
		padding: 2px 6px;
		border-radius: 3px;
		outline: none;
		width: 120px;
	}
</style>
