<script lang="ts">
	// Every `game` action, reachable from a phone at the venue.
	//
	// This rail is the risk mitigation: thresholds, board rotation, per-square
	// masks and the bar mapping are all live-editable, because none of their
	// right values are knowable until the hardware is on the table and no demo
	// survives a redeploy to fix a constant.

	import { squareName, type DetectMode, type GameState } from './types';

	interface Props {
		game: GameState;
		authed: boolean;
		deviceIds: string[];
		maskArmed: boolean;
		send: (action: string, extra?: Record<string, unknown>) => void;
		onArmMask: (armed: boolean) => void;
		onAuth: (password: string) => void;
	}
	let { game, authed, deviceIds, maskArmed, send, onArmMask, onAuth }: Props = $props();

	let open = $state(true);
	let password = $state('');
	let fenDraft = $state('');
	let evalDraft = $state(0);
	let positionDraft = $state('');

	// Board rotation and bar mapping are both "assume it's wrong" settings.
	let barSide = $state(0);
	let barHalf = $state(0);
	let barNode = $state(0);
	let barStrip = $state<'a' | 'b'>('a');
	let barPixel = $state(-1);

	const modes: DetectMode[] = ['auto', 'suggest', 'off'];
	const tun = $derived(game.tunables);

	function tune(key: string, value: number) {
		send('set_tunables', { [key]: value });
	}
</script>

<aside class="rail" class:collapsed={!open}>
	<div class="railhead">
		<h3>Operator</h3>
		<button class="chip" onclick={() => (open = !open)}>{open ? 'hide' : 'show'}</button>
	</div>

	{#if open}
		{#if !authed}
			<section class="card">
				<h3>Sign in</h3>
				<form
					onsubmit={(e) => {
						e.preventDefault();
						onAuth(password);
						password = '';
					}}
				>
					<input type="password" bind:value={password} placeholder="admin password" />
					<button class="chip" type="submit">unlock</button>
				</form>
				<p class="note">Spectator screens watch without this.</p>
			</section>
		{:else}
			<section class="card">
				<div class="cardhead">
					<h3>Game</h3>
					<span class="stat">{game.phase}</span>
				</div>
				<div class="row">
					<button class="chip" onclick={() => send('new_game')}>new game</button>
					<button class="chip" onclick={() => send('start')}>start</button>
					<button class="chip" onclick={() => send('undo')}>undo</button>
					<button class="chip" onclick={() => send('resync')}>resync</button>
					<button class="chip" onclick={() => send('rescore')}>rescore</button>
					<button class="chip bad" onclick={() => send('abort')}>abort</button>
				</div>
				<div class="row">
					<input
						bind:value={positionDraft}
						placeholder="position_id (rehearsed opener)"
						class="grow"
					/>
					<button
						class="chip"
						onclick={() => send('new_game', { position_id: positionDraft })}
						disabled={!positionDraft}>deal it</button
					>
				</div>
				<p class="note">
					deck: {game.deck.count} from {game.deck.source}{game.deck.skipped
						? ` · ${game.deck.skipped} skipped`
						: ''}
				</p>
			</section>

			<section class="card">
				<div class="cardhead">
					<h3>Detection</h3>
					<span class="stat" class:bad={!game.detect.sensors_live}>
						{game.detect.sensors_live ? 'sensors live' : 'no sensors'}
					</span>
				</div>
				<div class="row">
					{#each modes as mode (mode)}
						<button
							class="chip"
							class:active={game.detect.mode === mode}
							onclick={() => send('set_detect', { mode })}>{mode}</button
						>
					{/each}
				</div>
				<p class="note">
					<!-- The one control worth reaching for the moment auto misfires
					     on stage: still feels magic, cannot be wrong. -->
					`suggest` proposes and waits for a tap. `off` is pure click-to-move.
				</p>

				<div class="row">
					<button class="chip" class:arm={maskArmed} onclick={() => onArmMask(!maskArmed)}>
						{maskArmed ? 'click a square…' : 'mask a square'}
					</button>
					{#each game.detect.masked as square (square)}
						<button
							class="chip bad"
							onclick={() => send('mask_square', { square, masked: false })}
							title="unmask">{squareName(square)} ✕</button
						>
					{/each}
				</div>

				<label class="slider">
					<span>settle {tun.settle_ms} ms</span>
					<input
						type="range"
						min="200"
						max="2000"
						step="50"
						value={tun.settle_ms}
						oninput={(e) => tune('settle_ms', Number(e.currentTarget.value))}
					/>
				</label>
				<label class="slider">
					<span>auto-start hold {tun.autostart_stable_ms} ms</span>
					<input
						type="range"
						min="500"
						max="5000"
						step="100"
						value={tun.autostart_stable_ms}
						oninput={(e) => tune('autostart_stable_ms', Number(e.currentTarget.value))}
					/>
				</label>
				<label class="slider">
					<span>tier-3 max distance {tun.tier3_max_distance.toFixed(1)}</span>
					<input
						type="range"
						min="0"
						max="4"
						step="0.5"
						value={tun.tier3_max_distance}
						oninput={(e) => tune('tier3_max_distance', Number(e.currentTarget.value))}
					/>
				</label>
				<label class="slider">
					<span>tier-3 margin {tun.tier3_margin.toFixed(1)}</span>
					<input
						type="range"
						min="0"
						max="4"
						step="0.5"
						value={tun.tier3_margin}
						oninput={(e) => tune('tier3_margin', Number(e.currentTarget.value))}
					/>
				</label>
				<label class="slider">
					<span>unknown tolerance {tun.unknown_tolerance}</span>
					<input
						type="range"
						min="0"
						max="8"
						step="1"
						value={tun.unknown_tolerance}
						oninput={(e) => tune('unknown_tolerance', Number(e.currentTarget.value))}
					/>
				</label>
			</section>

			<section class="card">
				<div class="cardhead">
					<h3>Board</h3>
					<span class="stat">{game.device_id ?? 'unbound'}</span>
				</div>
				<div class="row">
					{#each deviceIds as id (id)}
						<button
							class="chip"
							class:active={game.device_id === id}
							onclick={() => send('bind_device', { device_id: id })}>{id}</button
						>
					{/each}
					{#if !deviceIds.length}<span class="note">no device connected</span>{/if}
				</div>
				<div class="row">
					<span class="label">rotation</span>
					{#each [0, 90, 180, 270] as degrees (degrees)}
						<button
							class="chip"
							class:active={game.detect.rotation === degrees}
							onclick={() => send('set_rotation', { degrees })}>{degrees}°</button
						>
					{/each}
				</div>
				<p class="note">
					squares: {game.lighting.squares}{game.lighting.bars_supported
						? ''
						: ' · edge bars unsupported'}
				</p>
			</section>

			<section class="card">
				<h3>Edge bars</h3>
				<!-- Which half-bar lands on which side depends on a hardware bodge
				     and is unknowable until game time, so this is a form to fill in
				     rather than a constant to guess. -->
				<div class="row">
					<label class="mini"
						>side<select bind:value={barSide}>
							{#each [0, 1, 2, 3] as s (s)}<option value={s}>{s}</option>{/each}
						</select></label
					>
					<label class="mini"
						>half<select bind:value={barHalf}>
							<option value={0}>0</option><option value={1}>1</option>
						</select></label
					>
					<label class="mini"
						>node<select bind:value={barNode}>
							{#each [0, 1, 2, 3] as n (n)}<option value={n}>{n}</option>{/each}
						</select></label
					>
					<label class="mini"
						>strip<select bind:value={barStrip}>
							<option value="a">A</option><option value="b">B</option>
						</select></label
					>
				</div>
				<div class="row">
					<button
						class="chip"
						onclick={() =>
							send('bars_test', {
								node: barNode,
								strip: barStrip,
								...(barPixel >= 0 ? { pixel: barPixel } : {})
							})}>light it</button
					>
					<label class="mini"
						>pixel<select bind:value={barPixel}>
							<option value={-1}>all</option>
							{#each [0, 1, 2, 3, 4, 5, 6, 7] as p (p)}<option value={p}>{p}</option>{/each}
						</select></label
					>
					<button
						class="chip"
						onclick={() =>
							send('bars_map', {
								side: barSide,
								half: barHalf,
								node: barNode,
								strip: barStrip
							})}>assign</button
					>
					<button
						class="chip"
						onclick={() =>
							send('bars_map', {
								side: barSide,
								half: barHalf,
								reversed: !(game.lighting.bar_map?.[barSide]?.[barHalf]?.reversed ?? false)
							})}>flip direction</button
					>
				</div>
			</section>

			<section class="card">
				<h3>Last word</h3>
				<div class="row">
					<span class="label">eval</span>
					<input type="number" bind:value={evalDraft} step="10" class="num" />
					<button class="chip" onclick={() => send('set_eval', { cp: evalDraft })}>decree</button>
				</div>
				<div class="row">
					<span class="label">winner</span>
					{#each ['white', 'black', 'draw'] as winner (winner)}
						<button class="chip" onclick={() => send('end', { winner })}>{winner}</button>
					{/each}
				</div>
				<div class="row">
					<input bind:value={fenDraft} placeholder="FEN — overwrite the position" class="grow" />
					<button
						class="chip"
						onclick={() => send('set_fen', { fen: fenDraft })}
						disabled={!fenDraft}>set</button
					>
				</div>
				<div class="row">
					<button
						class="chip"
						class:active={!!game.autopilot}
						onclick={() => send('autopilot', { on: !game.autopilot, interval_ms: 4000 })}
					>
						autopilot {game.autopilot ? 'on' : 'off'}
					</button>
					<span class="note">plays both sides — the rescue when nothing physical works</span>
				</div>
			</section>
		{/if}
	{/if}
</aside>

<style>
	.rail {
		display: flex;
		flex-direction: column;
		gap: 12px;
		width: 320px;
		max-height: 100%;
		overflow-y: auto;
	}
	.rail.collapsed {
		width: auto;
	}
	.railhead {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
	}
	.railhead h3 {
		margin: 0;
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.12em;
		text-transform: uppercase;
		color: var(--color-fg-dim);
	}

	.row {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
		margin-bottom: 8px;
	}
	.row:last-child {
		margin-bottom: 0;
	}
	.label {
		font-size: 10px;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--color-fg-faint);
	}

	input,
	select {
		font-family: var(--font-mono);
		font-size: 11px;
		color: var(--color-fg);
		background: var(--color-surface-2);
		border: 1px solid var(--color-line);
		border-radius: 6px;
		padding: 5px 7px;
		min-width: 0;
	}
	input.grow {
		flex: 1 1 140px;
	}
	input.num {
		width: 78px;
	}
	form {
		display: flex;
		gap: 6px;
	}
	form input {
		flex: 1;
	}

	.mini {
		display: flex;
		align-items: center;
		gap: 4px;
		font-size: 10px;
		color: var(--color-fg-faint);
	}
	.slider {
		display: flex;
		flex-direction: column;
		gap: 3px;
		margin-bottom: 8px;
		font-family: var(--font-mono);
		font-size: 10px;
		color: var(--color-fg-dim);
	}
	.slider input {
		width: 100%;
		padding: 0;
	}
</style>
