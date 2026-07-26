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
		type EventData,
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
			ws.teardown();
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

	// A frozen board that looks live is what makes transient faults unreadable, so
	// the rendered position is labelled the moment either link in the chain drops.
	const stale = $derived.by((): string | null => {
		if (!selected) return null;
		if (!ws.connected) return 'link down';
		return selected.connected ? null : 'board offline';
	});

	function submitAuth(e: Event) {
		e.preventDefault();
		if (password) ws.auth(password);
		adminOpen = false;
	}

	let probeFlash = $state<number | null>(null);
	let probeTimer: ReturnType<typeof setTimeout> | null = null;

	function onSquare(i: number) {
		if (!ws.authed || !selected) return;
		if (!ws.probe(selected.device_id, i)) return;
		// Mirror the physical blue flash on screen so a dead LED chain is still
		// distinguishable from a dead command path.
		probeFlash = null;
		if (probeTimer) clearTimeout(probeTimer);
		requestAnimationFrame(() => (probeFlash = i));
		probeTimer = setTimeout(() => (probeFlash = null), 1300);
	}

	// Calibration requires an empty board; arm-then-confirm instead of a modal.
	let calArm = $state<number | 'all' | null>(null);
	let calArmTimer: ReturnType<typeof setTimeout> | null = null;

	function onCalibrate(node: number | 'all') {
		if (!ws.authed || !selected) return;
		if (calArm !== node) {
			calArm = node;
			if (calArmTimer) clearTimeout(calArmTimer);
			calArmTimer = setTimeout(() => (calArm = null), 4000);
			return;
		}
		calArm = null;
		if (calArmTimer) clearTimeout(calArmTimer);
		ws.calibrate(selected.device_id, node);
	}

	function calLabel(n: number): string {
		const c = selected?.calibration[n];
		if (!c) return 'cal';
		if (c.active) return `${c.percent}%`;
		if (c.ok === true) return 'ok';
		if (c.ok === false) return 'fail';
		return 'cal';
	}

	function onWave() {
		if (selected) ws.wave(selected.device_id);
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
	// Both the ESP and the nodes report uptime as raw millis().
	function dur(ms: number | undefined): string {
		if (ms == null) return '—';
		const s = Math.floor(ms / 1000);
		const h = Math.floor(s / 3600);
		const m = Math.floor((s % 3600) / 60);
		if (h) return `${h}h ${m}m`;
		return m ? `${m}m` : `${s}s`;
	}
	function volts(mv: number | undefined): string {
		return mv == null ? '—' : `${(mv / 1000).toFixed(2)}V`;
	}
	// Only extended firmware reports these; legacy nodes get no stats line at all.
	function hasNodeStats(d: EventData | undefined): d is EventData {
		if (!d) return false;
		return (
			d.node_uptime_ms != null ||
			d.last_scan_ms != null ||
			d.event_depth != null ||
			d.node_rx_good != null ||
			d.node_rx_bad != null ||
			d.node_event_overflow != null ||
			d.supply_mv != null ||
			!!d.reboots ||
			!!d.timeouts
		);
	}
	const uptimeMs = $derived(ds?.uptime_ms ?? ds?.uptime);
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
				{stale}
				admin={ws.authed}
				{probeFlash}
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
							<dd class="tnum">{ds?.rssi != null ? `${ds.rssi} dBm` : '—'}</dd>
						</div>
						<div>
							<dt>heap</dt>
							<dd class="tnum">{kb(ds?.heap)}</dd>
						</div>
						<div>
							<dt>uptime</dt>
							<dd class="tnum">{dur(uptimeMs)}</dd>
						</div>
						{#if ds?.uart_good != null || ds?.uart_bad != null}
							<div>
								<dt>uart</dt>
								<dd class="tnum" class:warn={!!(ds?.uart_bad || ds?.uart_timeouts)}>
									{ds?.uart_good ?? 0} ok · {ds?.uart_bad ?? 0} bad · {ds?.uart_timeouts ?? 0} to
								</dd>
							</div>
						{/if}
						{#if ds?.ws_send_failed != null || ds?.events_dropped_offline != null}
							<div>
								<dt>ws drops</dt>
								<dd
									class="tnum"
									class:warn={!!(ds?.ws_send_failed || ds?.events_dropped_offline)}
									title="sendTXT failures · events discarded while the socket was not welcomed"
								>
									{ds?.ws_send_failed ?? 0} send · {ds?.events_dropped_offline ?? 0} offline
								</dd>
							</div>
						{/if}
						{#if ds?.snapshot_repairs != null}
							<div>
								<dt>repairs</dt>
								<dd
									class="tnum"
									class:warn={!!ds.snapshot_repairs}
									title="squares corrected by snapshot reconciliation"
								>
									{ds.snapshot_repairs}
								</dd>
							</div>
						{/if}
					</dl>
				</div>

				<div class="card">
					<div class="cardhead">
						<h3>Quadrants</h3>
						<div class="chips">
							<button
								class="chip"
								class:active={ws.waving}
								disabled={!ws.authed || !selected}
								onclick={onWave}
								title="sweep a lit file left to right, then a lit rank bottom to top"
							>
								{ws.waving ? 'sweeping' : 'wave'}
							</button>
							<button
								class="chip"
								class:arm={calArm === 'all'}
								disabled={!ws.authed || !selected}
								onclick={() => onCalibrate('all')}
								title="calibrate every online quadrant (board must be empty)"
							>
								{calArm === 'all' ? 'board empty?' : 'calibrate all'}
							</button>
						</div>
					</div>
					<ul class="nodes">
						{#each [0, 1, 2, 3] as n (n)}
							{@const h = nodeHealth(selected?.node_status[n] ?? null)}
							{@const d = selected?.node_status[n]?.data}
							{@const cal = selected?.calibration[n]}
							<li>
								<div class="nrow">
									<span class="ring {h}" class:pulse={cal?.active}></span>
									<span class="nname tnum">{nodeLabel[n]}</span>
									<span class="nstate {h}">{cal?.active ? 'calibrating' : h}</span>
									<span class="nfw tnum">{d?.firmware ?? ''}</span>
									<button
										class="chip ncal"
										class:arm={calArm === n}
										class:bad={cal?.ok === false}
										disabled={!ws.authed || h === 'offline' || h === 'unseen' || cal?.active}
										onclick={() => onCalibrate(n)}
										title={cal?.ok === false
											? `calibration failed: ${cal.reason ?? 'unknown'}`
											: 'calibrate this quadrant (empty board)'}
									>
										{calArm === n ? 'empty?' : calLabel(n)}
									</button>
								</div>
								<!-- Node-reported health. rx-bad and ovf are the two that mean lost data. -->
								{#if hasNodeStats(d)}
									<div class="nstats tnum">
										{#if d.node_uptime_ms != null}<span title="node uptime"
												>{dur(d.node_uptime_ms)}</span
											>{/if}
										{#if d.reboots}<span class="warn" title="observed node reboots"
												>rb {d.reboots}</span
											>{/if}
										{#if d.last_scan_ms != null}<span title="last full scan"
												>scan {d.last_scan_ms}ms</span
											>{/if}
										{#if d.event_depth != null}<span title="events queued on the node"
												>ev {d.event_depth}</span
											>{/if}
										{#if d.supply_mv != null}<span title="node supply">{volts(d.supply_mv)}</span
											>{/if}
										{#if d.node_rx_good != null || d.node_rx_bad != null}
											<span
												class:warn={!!d.node_rx_bad}
												title="frames decoded / rejected by the node"
											>
												rx {d.node_rx_good ?? 0}/{d.node_rx_bad ?? 0}
											</span>
										{/if}
										{#if d.node_event_overflow != null}
											<span
												class:warn={!!d.node_event_overflow}
												title="sensor events dropped by a full node queue"
											>
												ovf {d.node_event_overflow}
											</span>
										{/if}
										{#if d.timeouts}<span class="warn" title="bus timeouts">to {d.timeouts}</span
											>{/if}
									</div>
								{/if}
							</li>
						{/each}
					</ul>
					{#if ws.authed}
						<p class="note">
							wave sweeps blue by file (a&rarr;h), then amber by rank (1&rarr;8). Every square
							should light exactly once per pass, in order; one lighting out of step is a board-map
							fault, not a calibration fault.
						</p>
					{:else}
						<p class="note">authenticate as admin to calibrate</p>
					{/if}
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
	.chips {
		display: flex;
		gap: 6px;
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
	.chip.arm {
		color: var(--color-warn);
		border-color: color-mix(in srgb, var(--color-warn) 55%, var(--color-line));
		background: color-mix(in srgb, var(--color-warn) 12%, transparent);
	}
	.chip.bad {
		color: var(--color-fault);
		border-color: color-mix(in srgb, var(--color-fault) 45%, var(--color-line));
	}
	.ncal {
		margin-left: 8px;
		min-width: 5ch;
	}
	.ring.pulse {
		animation: cal-pulse 1s ease-in-out infinite;
	}
	@keyframes cal-pulse {
		50% {
			opacity: 0.35;
		}
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
	dd.warn {
		color: var(--color-warn);
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
		flex-direction: column;
		gap: 3px;
		font-family: var(--font-mono);
		font-size: 11.5px;
	}
	.nrow {
		display: flex;
		align-items: center;
		gap: 9px;
	}
	.nstats {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
		padding-left: 17px;
		font-size: 10px;
		color: var(--color-fg-ghost);
	}
	.nstats .warn {
		color: var(--color-warn);
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
	.nodes .chip {
		flex: none;
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
