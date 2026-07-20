<script lang="ts">
	import Board from '$lib/Board.svelte';
	import Console from '$lib/Console.svelte';
	import { ws } from '$lib/ws.svelte';
	import {
		nodeHealth,
		adcToVolts,
		FULL_SWING_COUNTS,
		POLARITY_GRADIENT,
		type DeviceState,
		type Envelope,
		type SquareState
	} from '$lib/types';

	const emptySquares: SquareState[] = Array.from({ length: 64 }, () => 'empty');
	const emptyValid: boolean[] = Array.from({ length: 64 }, () => true);
	const emptyNodes: (Envelope | null)[] = [null, null, null, null];

	let now = $state(Date.now());
	let debug = $state(
		typeof location !== 'undefined' && new URLSearchParams(location.search).has('debug')
	);
	let adminOpen = $state(false);
	let password = $state('');
	let heatmap = $state(
		typeof location !== 'undefined' && new URLSearchParams(location.search).has('heatmap')
	);

	$effect(() => {
		ws.connect();
		const t = setInterval(() => (now = Date.now()), 1000);
		const onKey = (e: KeyboardEvent) => {
			const el = e.target as HTMLElement | null;
			if (el && /^(INPUT|TEXTAREA)$/.test(el.tagName)) return;
			if (e.key === 'd' || e.key === 'D') debug = !debug;
		};
		window.addEventListener('keydown', onKey);
		return () => {
			clearInterval(t);
			window.removeEventListener('keydown', onKey);
		};
	});

	// Prefer a connected device; fall back to the most recent known one.
	const selected = $derived.by((): DeviceState | null => {
		const ids = ws.order;
		if (!ids.length) return null;
		const online = ids.find((id) => ws.devices[id]?.connected);
		return ws.devices[online ?? ids[0]] ?? null;
	});

	const rawAdc = $derived(selected?.raw_scan?.data?.raw_adc ?? null);
	const baselineAdc = $derived(selected?.raw_scan?.data?.baseline_adc ?? null);
	const rawScan = $derived(selected?.raw_scan ?? null);

	// Diverging colour full-scale is anchored to the physical conditioned swing
	// (±~307 counts ≈ ±1.5 V), so a square's colour maps to real polarity
	// magnitude and stays stable frame-to-frame while streaming.
	const heatSpan = FULL_SWING_COUNTS;
	const heatSpanVolts = adcToVolts(heatSpan) ?? 0;

	const nodesOnline = $derived.by(() => {
		if (!selected) return 0;
		return selected.node_status.filter((n) => {
			const h = nodeHealth(n);
			return h !== 'offline' && h !== 'unseen';
		}).length;
	});

	const ds = $derived(selected?.device_status?.data ?? null);
	const deviceRssi = $derived(ds?.rssi ?? ds?.wifi_rssi);
	const deviceHeap = $derived(ds?.heap ?? ds?.free_heap);

	function submitAuth(e: Event) {
		e.preventDefault();
		if (password) ws.auth(password);
		adminOpen = false;
	}

	function onSquare(i: number) {
		if (ws.authed && selected) ws.probe(selected.device_id, i);
	}

	// Streaming and the heatmap go together — turning the live scan on lights it up.
	function toggleStream() {
		if (!selected) return;
		const on = !ws.streaming;
		ws.setStream(selected.device_id, on);
		if (on) heatmap = true;
	}

	function ageLabel(dev: DeviceState): string {
		if (!dev.lastEventAt) return '—';
		const s = Math.max(0, Math.round((now - dev.lastEventAt) / 1000));
		return s < 1 ? 'live' : `${s}s ago`;
	}

	function kb(n: number | undefined): string {
		return n == null ? '—' : `${(n / 1024).toFixed(0)} KB`;
	}
	function dur(s: number | undefined): string {
		if (s == null) return '—';
		const h = Math.floor(s / 3600);
		const m = Math.floor((s % 3600) / 60);
		return h ? `${h}h ${m}m` : `${m}m`;
	}
	const nodeLabel = ['n0', 'n1', 'n2', 'n3'];
</script>

<svelte:head><title>Arcade Chess</title></svelte:head>

<div class="app" class:debug>
	<header>
		<div class="brand">
			<span class="mark"></span>
			<span class="word">ARCADE&nbsp;CHESS</span>
			<span class="sub">bring-up</span>
		</div>
		<div class="spacer"></div>
		<div class="status" class:muted={!selected}>
			<span class="dot" class:up={ws.connected}></span>
			{#if selected}
				<span class="tnum">{selected.device_id}</span>
				<span class="div">/</span>
				<span>{ageLabel(selected)}</span>
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
						autocomplete="off"
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

	<div class="stage" class:debug>
		<div class="board-slot">
			<Board
				squares={selected?.squares ?? emptySquares}
				valid={selected?.valid ?? emptyValid}
				nodeStatus={selected?.node_status ?? emptyNodes}
				rawAdc={debug ? rawAdc : null}
				{baselineAdc}
				{debug}
				{heatmap}
				{heatSpan}
				admin={ws.authed}
				{onSquare}
			/>
			{#if !debug}
				<div class="caption" class:muted={!selected}>
					{#if selected}
						<span>{nodesOnline}/4 quadrants online</span>
						{#if ws.authed}
							<span class="div">·</span><span class="hint">tap a square to probe</span>
						{/if}
					{:else}
						<span>waiting for a board to connect</span>
					{/if}
				</div>
			{/if}
		</div>

		{#if debug}
			<aside class="rail">
				<div class="card">
					<h3>Device</h3>
					<dl>
						<div>
							<dt>id</dt>
							<dd class="tnum">{selected?.device_id ?? '—'}</dd>
						</div>
						<div>
							<dt>link</dt>
							<dd class={selected?.connected ? 'ok' : 'bad'}>
								{selected?.connected ? 'connected' : 'offline'}
							</dd>
						</div>
						<div>
							<dt>rssi</dt>
							<dd class="tnum">{deviceRssi != null ? `${deviceRssi} dBm` : '—'}</dd>
						</div>
						<div>
							<dt>heap</dt>
							<dd class="tnum">{kb(deviceHeap)}</dd>
						</div>
						<div>
							<dt>uptime</dt>
							<dd class="tnum">{dur(ds?.uptime)}</dd>
						</div>
					</dl>
				</div>

				<div class="card">
					<h3>Quadrants</h3>
					<ul class="nodes">
						{#each [0, 1, 2, 3] as n (n)}
							{@const h = nodeHealth(selected?.node_status[n] ?? null)}
							{@const d = selected?.node_status[n]?.data}
							<li>
								<span class="ring {h}"></span>
								<span class="nname tnum">{nodeLabel[n]}</span>
								<span class="nstate {h}">{h}</span>
								<span class="nfw tnum">{d?.firmware ?? ''}</span>
							</li>
						{/each}
					</ul>
				</div>

				<div class="card">
					<div class="cardhead">
						<h3>Voltages</h3>
						<button
							class="chip"
							class:active={heatmap}
							disabled={!rawScan}
							onclick={() => (heatmap = !heatmap)}>heatmap</button
						>
					</div>
					<div class="scanmeta tnum">
						{#if rawScan}
							scan {rawScan.data?.scan_id ?? '—'} · {rawScan.data?.complete
								? 'complete'
								: 'partial'} · ±{(heatSpanVolts ?? 0).toFixed(2)}V full-scale
						{:else}
							no scan yet
						{/if}
					</div>
					<div class="btnrow">
						<button
							class="btn"
							disabled={!ws.authed || !selected}
							onclick={() => selected && ws.rawScan(selected.device_id)}>Scan once</button
						>
						<button
							class="btn"
							class:active={ws.streaming}
							disabled={!ws.authed || !selected}
							onclick={toggleStream}>{ws.streaming ? 'Streaming' : 'Stream'}</button
						>
					</div>
					{#if !ws.authed}<p class="note">authenticate as admin to drive scans</p>{/if}
					<!-- Diverging legend: negative polarity ← centre → positive polarity. -->
					<div class="scale">
						<span class="pole neg">− neg</span>
						<span class="bar" style:background={POLARITY_GRADIENT}></span>
						<span class="pole pos">pos +</span>
					</div>
				</div>
			</aside>

			<div class="console-slot">
				<Console device={selected} />
			</div>
		{/if}
	</div>
</div>

<style>
	.app {
		min-height: 100vh;
		display: grid;
		grid-template-rows: auto 1fr;
		position: relative;
		z-index: 1;
	}

	/* ── Header ───────────────────────────────────────────────────────────────*/
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

	/* ── Stage ────────────────────────────────────────────────────────────────*/
	.stage {
		display: grid;
		place-items: center;
		padding: clamp(16px, 4vw, 44px);
	}
	.board-slot {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 16px;
	}
	.caption {
		display: flex;
		align-items: center;
		gap: 9px;
		font-family: var(--font-mono);
		font-size: 12px;
		color: var(--color-fg-faint);
	}
	.caption .div {
		color: var(--color-fg-ghost);
	}
	.caption .hint {
		color: var(--color-probe);
	}

	/* Debug: instrument dashboard — board, right rail, bottom console. */
	.stage.debug {
		grid-template-columns: minmax(0, 1fr) 336px;
		grid-template-rows: minmax(0, 1fr) 216px;
		grid-template-areas:
			'board rail'
			'console rail';
		gap: 18px;
		align-items: stretch;
		place-items: stretch;
	}
	.stage.debug .board-slot {
		grid-area: board;
		justify-content: center;
	}
	.stage.debug .rail {
		grid-area: rail;
	}
	.stage.debug .console-slot {
		grid-area: console;
		min-height: 0;
	}

	.rail {
		display: flex;
		flex-direction: column;
		gap: 14px;
		overflow-y: auto;
	}
	.card {
		background: color-mix(in srgb, var(--color-surface) 82%, transparent);
		border: 1px solid var(--color-line);
		border-radius: 12px;
		padding: 13px 15px;
		backdrop-filter: blur(10px);
	}
	.card h3 {
		margin: 0 0 10px;
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.12em;
		text-transform: uppercase;
		color: var(--color-fg-dim);
	}
	.cardhead {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: 10px;
	}
	.cardhead h3 {
		margin: 0;
	}
	.chip {
		height: 22px;
		padding: 0 10px;
		font-family: var(--font-mono);
		font-size: 10px;
		letter-spacing: 0.04em;
		color: var(--color-fg-faint);
		background: var(--color-surface-2);
		border: 1px solid var(--color-line);
		border-radius: 999px;
		cursor: pointer;
		transition:
			color 0.15s ease,
			border-color 0.15s ease,
			background 0.15s ease;
	}
	.chip:hover:not(:disabled) {
		color: var(--color-fg);
	}
	.chip:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}
	.chip.active {
		color: var(--color-fg);
		border-color: color-mix(in srgb, var(--color-pos) 45%, var(--color-line));
		background: color-mix(in srgb, var(--color-pos) 14%, transparent);
	}
	dl {
		margin: 0;
		display: flex;
		flex-direction: column;
		gap: 7px;
	}
	dl > div {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
		font-size: 12px;
	}
	dt {
		color: var(--color-fg-faint);
		font-family: var(--font-mono);
		font-size: 11px;
	}
	dd {
		margin: 0;
		color: var(--color-fg);
		font-family: var(--font-mono);
		font-size: 11.5px;
	}
	dd.ok {
		color: var(--color-live);
	}
	dd.bad {
		color: var(--color-fault);
	}

	.nodes {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.nodes li {
		display: flex;
		align-items: center;
		gap: 9px;
		font-family: var(--font-mono);
		font-size: 11.5px;
	}
	.ring {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		flex: none;
		background: var(--color-fg-ghost);
	}
	.ring.healthy {
		background: var(--color-live);
		box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-live) 16%, transparent);
	}
	.ring.uncalibrated {
		background: var(--color-warn);
		box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-warn) 16%, transparent);
	}
	.ring.offline {
		background: var(--color-fault);
	}
	.nname {
		color: var(--color-fg);
		min-width: 3ch;
	}
	.nstate {
		color: var(--color-fg-faint);
		text-transform: capitalize;
	}
	.nstate.healthy {
		color: var(--color-live);
	}
	.nstate.uncalibrated {
		color: var(--color-warn);
	}
	.nstate.offline {
		color: var(--color-fault);
	}
	.nfw {
		margin-left: auto;
		color: var(--color-fg-ghost);
		font-size: 10px;
	}

	.scanmeta {
		font-family: var(--font-mono);
		font-size: 10.5px;
		color: var(--color-fg-faint);
		margin-bottom: 10px;
	}
	.btnrow {
		display: flex;
		gap: 8px;
	}
	.btn {
		flex: 1;
		height: 30px;
		font-family: var(--font-mono);
		font-size: 11px;
		color: var(--color-fg-dim);
		background: var(--color-surface-2);
		border: 1px solid var(--color-line);
		border-radius: 8px;
		cursor: pointer;
		transition:
			color 0.15s ease,
			border-color 0.15s ease,
			background 0.15s ease;
	}
	.btn:hover:not(:disabled) {
		color: var(--color-fg);
		border-color: var(--color-fg-faint);
	}
	.btn:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}
	.btn.active {
		color: var(--color-probe);
		border-color: color-mix(in srgb, var(--color-probe) 50%, var(--color-line));
		background: color-mix(in srgb, var(--color-probe) 12%, transparent);
	}
	.note {
		margin: 9px 0 0;
		font-size: 10.5px;
		color: var(--color-fg-ghost);
	}
	.scale {
		display: flex;
		align-items: center;
		gap: 8px;
		margin-top: 12px;
		font-family: var(--font-mono);
		font-size: 9.5px;
		color: var(--color-fg-ghost);
	}
	.scale .bar {
		flex: 1;
		height: 6px;
		border-radius: 3px;
	}
	.scale .pole {
		flex: none;
	}
	.scale .pole.neg {
		color: color-mix(in srgb, var(--color-neg) 70%, var(--color-fg-faint));
	}
	.scale .pole.pos {
		color: color-mix(in srgb, var(--color-pos) 70%, var(--color-fg-faint));
	}

	@media (max-width: 1080px) {
		.stage.debug {
			grid-template-columns: 1fr;
			grid-template-rows: auto auto auto;
			grid-template-areas:
				'board'
				'rail'
				'console';
		}
		.stage.debug .console-slot {
			height: 260px;
		}
	}
</style>
