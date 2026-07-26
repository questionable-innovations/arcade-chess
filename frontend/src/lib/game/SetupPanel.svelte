<script lang="ts">
	// Setup guidance, and the Start button.
	//
	// Start is always present and always works, and it says what it is
	// overriding. Auto-start is the nice path; Start is the one the demo
	// actually depends on.

	import { squareName, type GameState } from './types';

	interface Props {
		game: GameState;
		canControl: boolean;
		onStart: () => void;
	}
	let { game, canControl, onStart }: Props = $props();

	const setup = $derived(game.setup);
	const countdown = $derived(setup?.auto_start_in_ms ?? null);

	const override = $derived.by(() => {
		if (!setup) return '';
		const bits: string[] = [`${setup.placed} of ${setup.needed} placed`];
		if (setup.extra.length) bits.push(`${setup.extra.length} unexpected`);
		if (setup.unknown.length) bits.push(`${setup.unknown.length} sensor unknown`);
		if (!game.detect.sensors_live) bits.push('no live sensors');
		return bits.join(' · ');
	});
</script>

<section class="card setup">
	<div class="cardhead">
		<h3>Setup</h3>
		{#if countdown != null}
			<span class="stat ok tnum">starting in {Math.max(1, Math.ceil(countdown / 1000))}</span>
		{:else if game.detect.sensors_live}
			<span class="stat">watching the board</span>
		{:else}
			<span class="stat bad">manual mode</span>
		{/if}
	</div>

	<p class="hint">
		Place the pieces shown. Amber squares still need one; the board lights them too.
	</p>

	{#if setup}
		<dl class="counts">
			<div>
				<dt>placed</dt>
				<dd class="tnum">{setup.placed}/{setup.needed}</dd>
			</div>
			<div>
				<dt>missing</dt>
				<dd class="tnum">{setup.missing.length}</dd>
			</div>
			<div>
				<dt>unexpected</dt>
				<dd class="tnum">{setup.extra.length}</dd>
			</div>
			<div>
				<dt>unknown</dt>
				<dd class="tnum">{setup.unknown.length}</dd>
			</div>
		</dl>

		{#if setup.missing.length}
			<p class="squares">
				<span class="tag warn">needed</span>
				{setup.missing.map(squareName).join(' ')}
			</p>
		{/if}
		{#if setup.extra.length}
			<p class="squares">
				<span class="tag bad">remove</span>
				{setup.extra.map(squareName).join(' ')}
			</p>
		{/if}
		{#if setup.unknown.length}
			<p class="squares">
				<span class="tag dim">no sensor</span>
				{setup.unknown.map(squareName).join(' ')}
			</p>
		{/if}
	{/if}

	<button class="start" disabled={!canControl} onclick={onStart}>
		Start now
		{#if override}<span class="override">{override}</span>{/if}
	</button>
	{#if !canControl}
		<p class="note">Sign in on the admin rail to start.</p>
	{/if}
</section>

<style>
	.setup {
		min-width: min(260px, 100%);
	}
	.hint {
		margin: 0 0 10px;
		font-size: 12px;
		color: var(--color-fg-dim);
	}
	.counts {
		display: grid;
		grid-template-columns: repeat(2, 1fr);
		gap: 6px 14px;
		margin: 0 0 10px;
	}
	.counts div {
		display: flex;
		justify-content: space-between;
		gap: 8px;
	}
	dt {
		font-size: 11px;
		color: var(--color-fg-faint);
	}
	dd {
		margin: 0;
		font-family: var(--font-mono);
		font-size: 12px;
	}
	.squares {
		margin: 0 0 6px;
		font-family: var(--font-mono);
		font-size: 11px;
		color: var(--color-fg-dim);
		word-break: break-word;
	}
	.tag {
		display: inline-block;
		font-size: 9.5px;
		letter-spacing: 0.09em;
		text-transform: uppercase;
		margin-right: 8px;
	}
	.tag.warn {
		color: var(--color-warn);
	}
	.tag.bad {
		color: var(--color-fault);
	}
	.tag.dim {
		color: var(--color-fg-faint);
	}

	.start {
		width: 100%;
		min-height: 48px;
		margin-top: 8px;
		padding: 12px;
		display: flex;
		flex-direction: column;
		gap: 3px;
		align-items: center;
		font-family: inherit;
		font-size: 15px;
		font-weight: 600;
		color: var(--color-ink);
		background: var(--color-live);
		border: 0;
		border-radius: 9px;
		cursor: pointer;
	}
	.start:disabled {
		opacity: 0.35;
		cursor: not-allowed;
	}
	.override {
		font-family: var(--font-mono);
		font-size: 10px;
		font-weight: 400;
		opacity: 0.75;
	}
</style>
