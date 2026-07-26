<script lang="ts">
	// Ten seconds of operator attention, not a broken game.
	//
	// This is where every ambiguity the sensors cannot resolve lands: two legal
	// captures by the same piece with the journal lost, a settle that matches
	// nothing legal, or `suggest` mode where nothing auto-commits by design.

	import { squareName, type GameState } from './types';

	interface Props {
		game: GameState;
		canControl: boolean;
		onChoose: (uci: string) => void;
	}
	let { game, canControl, onChoose }: Props = $props();

	const choice = $derived(game.choice);
	const mismatch = $derived(game.detect.mismatch);
</script>

{#if choice}
	<section class="prompt" class:alarm={choice.kind === 'no_match'}>
		<p class="ask">{choice.prompt}</p>

		{#if choice.kind === 'no_match' && mismatch.length}
			<p class="diff">
				board and game disagree at
				<strong>{mismatch.map(squareName).join(', ')}</strong>
			</p>
		{/if}

		<div class="options">
			{#each choice.options as option (option.uci)}
				<button class="option" disabled={!canControl} onclick={() => onChoose(option.uci)}>
					<span class="san">{option.san}</span>
					<span class="uci">{option.uci}</span>
				</button>
			{/each}
			<button class="option none" disabled={!canControl} onclick={() => onChoose('')}>
				<span class="san">None of these</span>
				<span class="uci">leave the position alone</span>
			</button>
		</div>
	</section>
{/if}

<style>
	.prompt {
		display: flex;
		flex-direction: column;
		gap: 10px;
		/* Sits directly under the board and lines up with it, at every width. */
		width: 100%;
		max-width: 620px;
		max-width: min(78dvh, 620px);
		padding: 16px 18px;
		border-radius: 12px;
		background: color-mix(in srgb, var(--color-probe) 12%, var(--color-surface));
		border: 1px solid color-mix(in srgb, var(--color-probe) 45%, var(--color-line));
	}
	.prompt.alarm {
		background: color-mix(in srgb, var(--color-fault) 12%, var(--color-surface));
		border-color: color-mix(in srgb, var(--color-fault) 50%, var(--color-line));
	}
	.ask {
		margin: 0;
		font-size: 17px;
		font-weight: 600;
	}
	.diff {
		margin: 0;
		font-family: var(--font-mono);
		font-size: 11px;
		color: var(--color-fg-dim);
	}
	.options {
		display: flex;
		flex-wrap: wrap;
		gap: 9px;
	}
	.option {
		flex: 1 1 130px;
		display: flex;
		flex-direction: column;
		gap: 2px;
		align-items: center;
		min-height: 56px;
		justify-content: center;
		padding: 14px 12px;
		font-family: inherit;
		color: var(--color-fg);
		background: var(--color-surface-2);
		border: 1px solid var(--color-line);
		border-radius: 10px;
		cursor: pointer;
	}
	@media (hover: hover) {
		.option:hover:not(:disabled) {
			border-color: var(--color-probe);
		}
	}
	.option:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}
	.option.none {
		flex-basis: 100%;
	}
	.san {
		font-size: 22px;
		font-weight: 600;
	}
	.option.none .san {
		font-size: 14px;
		color: var(--color-fg-dim);
	}
	.uci {
		font-family: var(--font-mono);
		font-size: 10px;
		color: var(--color-fg-faint);
	}
</style>
