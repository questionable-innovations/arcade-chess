<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import AppHeader from '$lib/AppHeader.svelte';
	import Board from '$lib/Board.svelte';
	import Console from '$lib/Console.svelte';
	import DeviceCard from '$lib/DeviceCard.svelte';
	import QuadrantsCard from '$lib/QuadrantsCard.svelte';
	import VoltagesCard from '$lib/VoltagesCard.svelte';
	import { ws } from '$lib/ws.svelte';
	import {
		nodeHealth,
		adcToVolts,
		FULL_SWING_COUNTS,
		NODE_COUNT,
		SQUARE_COUNT,
		type DeviceState,
		type Envelope,
		type SquareState
	} from '$lib/types';

	const emptySquares: SquareState[] = Array.from({ length: SQUARE_COUNT }, () => 'empty');
	const emptyValid: boolean[] = Array.from({ length: SQUARE_COUNT }, () => true);
	const emptyNodes: (Envelope | null)[] = Array.from({ length: NODE_COUNT }, () => null);

	let now = $state(Date.now());
	let debug = $state(
		typeof location !== 'undefined' && new URLSearchParams(location.search).has('debug')
	);
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
			// The dashboard is the tool; the game is the point. One key away.
			if (e.key === 'g' || e.key === 'G') goto(resolve('/game'));
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

	// A frozen board that looks live is what makes transient faults unreadable, so
	// the rendered position is labelled the moment either link in the chain drops.
	const stale = $derived.by((): string | null => {
		if (!selected) return null;
		if (!ws.connected) return 'link down';
		return selected.connected ? null : 'board offline';
	});

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

	function onWave() {
		if (selected) ws.wave(selected.device_id);
	}

	function onScan() {
		if (selected) ws.rawScan(selected.device_id);
	}

	// Streaming and the heatmap go together — turning the live scan on lights it up.
	function toggleStream() {
		if (!selected) return;
		const on = !ws.streaming;
		ws.setStream(selected.device_id, on);
		if (on) heatmap = true;
	}
</script>

<svelte:head><title>Arcade Chess</title></svelte:head>

<div class="app" class:debug>
	<AppHeader device={selected} {now} bind:debug />

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
						<span>{nodesOnline}/{NODE_COUNT} quadrants online</span>
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
				<DeviceCard device={selected} />
				<QuadrantsCard device={selected} {calArm} {onCalibrate} {onWave} />
				<VoltagesCard
					device={selected}
					{heatSpanVolts}
					bind:heatmap
					{onScan}
					onToggleStream={toggleStream}
				/>
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
