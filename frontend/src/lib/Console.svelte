<script lang="ts">
	import { ws } from './ws.svelte';
	import type { DeviceState } from './types';

	let { device }: { device: DeviceState | null } = $props();
</script>

<section class="console" aria-label="event log">
	<header>
		<span class="title">console</span>
		{#if device}
			<span class="meta tnum">
				boot {device.bootId ?? '—'} · seq {device.seq < 0 ? '—' : device.seq}{#if device.gap}<span
						class="gap">gap</span
					>{/if}
			</span>
		{/if}
		<span class="count tnum">{ws.events.length}</span>
	</header>
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
