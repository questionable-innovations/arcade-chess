<script lang="ts">
	import { POLARITY_GRADIENT, type DeviceState } from './types';
	import { ws } from './ws.svelte';

	let {
		device,
		heatSpanVolts,
		heatmap = $bindable(),
		onScan,
		onToggleStream
	}: {
		device: DeviceState | null;
		/** Half-width of the diverging colour scale, in volts. */
		heatSpanVolts: number;
		heatmap: boolean;
		onScan: () => void;
		onToggleStream: () => void;
	} = $props();

	const rawScan = $derived(device?.raw_scan ?? null);
</script>

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
			scan {rawScan.data?.scan_id ?? '—'} · {rawScan.data?.complete ? 'complete' : 'partial'} · ±{heatSpanVolts.toFixed(
				2
			)}V full-scale
		{:else}
			no scan yet
		{/if}
	</div>
	<div class="btnrow">
		<button class="btn" disabled={!ws.authed || !device} onclick={onScan}>Scan once</button>
		<button
			class="btn"
			class:active={ws.streaming}
			disabled={!ws.authed || !device}
			onclick={onToggleStream}>{ws.streaming ? 'Streaming' : 'Stream'}</button
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

<style>
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
		border-radius: 6px;
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
</style>
