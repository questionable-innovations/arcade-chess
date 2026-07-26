<script lang="ts">
	import { dur, volts } from './format';
	import { nodeHealth, NODE_INDICES, type DeviceState, type EventData } from './types';
	import { ws } from './ws.svelte';

	let {
		device,
		calArm,
		onCalibrate,
		onWave
	}: {
		device: DeviceState | null;
		/** Which quadrant is armed and awaiting a confirming second press. */
		calArm: number | 'all' | null;
		onCalibrate: (node: number | 'all') => void;
		onWave: () => void;
	} = $props();

	function calLabel(n: number): string {
		const c = device?.calibration[n];
		if (!c) return 'cal';
		if (c.active) return `${c.percent}%`;
		if (c.ok === true) return 'ok';
		if (c.ok === false) return 'fail';
		return 'cal';
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

	const nodeLabel = NODE_INDICES.map((n) => `n${n}`);
</script>

<div class="card">
	<div class="cardhead">
		<h3>Quadrants</h3>
		<div class="chips">
			<button
				class="chip"
				class:active={ws.waving}
				disabled={!ws.authed || !device}
				onclick={onWave}
				title="sweep a lit file left to right, then a lit rank bottom to top"
			>
				{ws.waving ? 'sweeping' : 'wave'}
			</button>
			<button
				class="chip"
				class:arm={calArm === 'all'}
				disabled={!ws.authed || !device}
				onclick={() => onCalibrate('all')}
				title="calibrate every online quadrant (board must be empty)"
			>
				{calArm === 'all' ? 'board empty?' : 'calibrate all'}
			</button>
		</div>
	</div>
	<ul class="nodes">
		{#each NODE_INDICES as n (n)}
			{@const h = nodeHealth(device?.node_status[n] ?? null)}
			{@const d = device?.node_status[n]?.data}
			{@const cal = device?.calibration[n]}
			<li>
				<div class="nrow">
					<span class="nname tnum">{nodeLabel[n]}</span>
					<span class="nstate {h}" class:pulse={cal?.active}>
						{cal?.active ? 'calibrating' : h}
					</span>
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
						{#if d.node_uptime_ms != null}<span title="node uptime">{dur(d.node_uptime_ms)}</span
							>{/if}
						{#if d.reboots}<span class="warn" title="observed node reboots">rb {d.reboots}</span
							>{/if}
						{#if d.last_scan_ms != null}<span title="last full scan">scan {d.last_scan_ms}ms</span
							>{/if}
						{#if d.event_depth != null}<span title="events queued on the node"
								>ev {d.event_depth}</span
							>{/if}
						{#if d.supply_mv != null}<span title="node supply">{volts(d.supply_mv)}</span>{/if}
						{#if d.node_rx_good != null || d.node_rx_bad != null}
							<span class:warn={!!d.node_rx_bad} title="frames decoded / rejected by the node">
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
						{#if d.timeouts}<span class="warn" title="bus timeouts">to {d.timeouts}</span>{/if}
					</div>
				{/if}
			</li>
		{/each}
	</ul>
	{#if ws.authed}
		<p class="note">
			wave sweeps blue by file (a&rarr;h), then amber by rank (1&rarr;8). A square lighting out of
			order is a board-map fault, not a calibration fault.
		</p>
	{:else}
		<p class="note">authenticate as admin to calibrate</p>
	{/if}
</div>

<style>
	.chips {
		display: flex;
		gap: 6px;
	}
	.ncal {
		margin-left: 8px;
		min-width: 5ch;
	}
	.pulse {
		animation: cal-pulse 1s ease-in-out infinite;
	}
	@keyframes cal-pulse {
		50% {
			opacity: 0.35;
		}
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
		flex-wrap: wrap;
		gap: 6px 9px;
	}
	.nstats {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
		font-size: 10px;
		color: var(--color-fg-ghost);
	}
	.nstats .warn {
		color: var(--color-warn);
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
</style>
