<script lang="ts">
	import { ws } from './ws.svelte';
	import { messageTypeLabel, type DeviceState, type Envelope } from './types';

	let { device }: { device: DeviceState | null } = $props();
	let view = $state<'events' | 'bus'>('events');

	// Space the hex pairs so payload bytes can be read against docs/uart-api.md.
	function spacedHex(hex: string | undefined): string {
		if (!hex) return '';
		return hex.replace(/(..)/g, '$1 ').trimEnd();
	}

	function busLevel(env: Envelope): string {
		const r = env.data?.result ?? '';
		return r === 'ok' || r === 'sent' ? 'event' : 'warn';
	}
</script>

<section class="console" aria-label="event log">
	<header>
		<button class="title tab" class:on={view === 'events'} onclick={() => (view = 'events')}>
			console
		</button>
		<button class="title tab" class:on={view === 'bus'} onclick={() => (view = 'bus')}>
			bus{#if ws.tracing}<span class="livedot"></span>{/if}
		</button>
		{#if device}
			<span class="meta tnum">
				boot {device.bootId ?? '—'} · seq {device.seq < 0 ? '—' : device.seq}{#if device.gap}<span
						class="gap">gap</span
					>{/if}
			</span>
		{/if}
		{#if view === 'bus'}
			<button
				class="trace"
				class:on={ws.tracing}
				disabled={!ws.authed || !device}
				onclick={() => device && ws.setTrace(device.device_id, !ws.tracing)}
				title={ws.authed ? 'toggle UART frame capture' : 'authenticate as admin to trace'}
			>
				{ws.tracing ? 'tracing' : 'trace'}
			</button>
		{/if}
		<span class="count tnum">{view === 'bus' ? ws.busFrames.length : ws.events.length}</span>
	</header>
	{#if view === 'events'}
		<div class="stream">
			{#each ws.events as ev (ev.id)}
				<div class="row {ev.level}">
					<span class="at tnum">{ev.at}</span>
					<span class="txt">{ev.text}</span>
				</div>
			{:else}
				<div class="empty">no events yet</div>
			{/each}
		</div>
	{:else}
		<div class="stream">
			{#each ws.busFrames as frame (frame.id)}
				{@const env = frame.env}
				{@const d = env.data ?? {}}
				<div class="row {busLevel(env)}">
					<span class="at tnum">{env.at_ms ?? '·'}</span>
					<span class="txt">
						<span class="dir {d.direction}">{d.direction}</span>
						<span class="tnum">n{d.node}</span>
						<span class="tnum">#{d.uart_seq}</span>
						<span class="mt">{messageTypeLabel(d.message_type)}</span>
						<span class="res">{d.result}</span>
						{#if d.dropped}<span class="drop">+{d.dropped} dropped</span>{/if}
						{#if d.raw_hex}<span class="hex tnum">{spacedHex(d.raw_hex)}</span>{/if}
					</span>
				</div>
			{:else}
				<div class="empty">
					{ws.tracing ? 'waiting for frames…' : 'enable Trace in the rail to capture UART frames'}
				</div>
			{/each}
		</div>
	{/if}
</section>

<style>
	.console {
		display: flex;
		flex-direction: column;
		height: 100%;
		background: color-mix(in srgb, var(--color-surface) 82%, transparent);
		border: 1px solid var(--color-line);
		border-radius: 12px;
		overflow: hidden;
		backdrop-filter: blur(10px);
	}
	header {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 9px 12px;
		border-bottom: 1px solid var(--color-line-soft);
	}
	.title {
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.12em;
		text-transform: uppercase;
		color: var(--color-fg-dim);
	}
	.tab {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		border: 0;
		background: transparent;
		padding: 2px 4px;
		cursor: pointer;
		color: var(--color-fg-ghost);
	}
	.tab.on {
		color: var(--color-fg);
	}
	.livedot {
		width: 5px;
		height: 5px;
		border-radius: 50%;
		background: var(--color-live);
	}
	.trace {
		height: 20px;
		padding: 0 9px;
		font-family: var(--font-mono);
		font-size: 10px;
		letter-spacing: 0.05em;
		color: var(--color-fg-faint);
		background: var(--color-surface-2);
		border: 1px solid var(--color-line);
		border-radius: 999px;
		cursor: pointer;
	}
	.trace:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}
	.trace.on {
		color: var(--color-live);
		border-color: color-mix(in srgb, var(--color-live) 45%, var(--color-line));
		background: color-mix(in srgb, var(--color-live) 10%, transparent);
	}
	.dir {
		display: inline-block;
		min-width: 2ch;
		font-weight: 600;
	}
	.dir.tx {
		color: var(--color-probe);
	}
	.dir.rx {
		color: var(--color-live);
	}
	.mt {
		color: var(--color-fg);
	}
	.res {
		color: var(--color-fg-faint);
	}
	.drop {
		color: var(--color-warn);
	}
	.hex {
		color: var(--color-fg-faint);
		letter-spacing: 0.02em;
	}
	.meta {
		font-family: var(--font-mono);
		font-size: 10.5px;
		color: var(--color-fg-faint);
	}
	.gap {
		margin-left: 6px;
		padding: 0 5px;
		border-radius: 4px;
		background: color-mix(in srgb, var(--color-fault) 22%, transparent);
		color: var(--color-fault);
	}
	.count {
		margin-left: auto;
		font-family: var(--font-mono);
		font-size: 10px;
		color: var(--color-fg-ghost);
	}
	.stream {
		flex: 1;
		overflow-y: auto;
		padding: 6px 4px 8px;
		display: flex;
		flex-direction: column;
		gap: 1px;
	}
	.row {
		display: flex;
		gap: 10px;
		padding: 2px 10px;
		font-family: var(--font-mono);
		font-size: 11px;
		line-height: 1.55;
		border-left: 2px solid transparent;
		white-space: nowrap;
	}
	.row .at {
		color: var(--color-fg-ghost);
		flex: none;
		min-width: 6ch;
		text-align: right;
	}
	.row .txt {
		color: var(--color-fg-dim);
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.row.warn {
		border-left-color: var(--color-warn);
	}
	.row.warn .txt {
		color: color-mix(in srgb, var(--color-warn) 80%, var(--color-fg));
	}
	.row.error {
		border-left-color: var(--color-fault);
	}
	.row.error .txt {
		color: color-mix(in srgb, var(--color-fault) 78%, var(--color-fg));
	}
	.row.info .txt {
		color: var(--color-probe);
	}
	.empty {
		padding: 16px;
		text-align: center;
		font-family: var(--font-mono);
		font-size: 11px;
		color: var(--color-fg-ghost);
	}
	.stream::-webkit-scrollbar {
		width: 8px;
	}
	.stream::-webkit-scrollbar-thumb {
		background: var(--color-line);
		border-radius: 4px;
	}
</style>
