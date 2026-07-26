<script lang="ts">
	import { dur, kb } from './format';
	import type { DeviceState } from './types';

	let { device }: { device: DeviceState | null } = $props();

	const ds = $derived(device?.device_status?.data ?? null);
	const uptimeMs = $derived(ds?.uptime_ms ?? ds?.uptime);
</script>

<div class="card">
	<h3>Device</h3>
	<dl>
		<div>
			<dt>id</dt>
			<dd class="tnum">{device?.device_id ?? '—'}</dd>
		</div>
		<div>
			<dt>link</dt>
			<dd class={device?.connected ? 'ok' : 'bad'}>
				{device?.connected ? 'connected' : 'offline'}
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

<style>
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
</style>
