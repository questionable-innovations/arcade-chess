<script lang="ts">
	// Every `game` action, reachable from a phone at the venue.
	//
	// This rail is the risk mitigation: thresholds, board rotation, per-square
	// masks and the bar mapping are all live-editable, because none of their
	// right values are knowable until the hardware is on the table and no demo
	// survives a redeploy to fix a constant.

	import { squareName, type DetectMode, type GameState, type Setting } from './types';

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

	// The AVR's own EEPROM keys. The transport always existed; only a way to
	// reach it from the web did not, which left the most venue-dependent values
	// in the whole system — sensor thresholds, LED brightness — behind a USB
	// cable and a crouch.
	const NODE_KEYS = [
		{ key: 1, label: 'enter threshold', min: 10, max: 400 },
		{ key: 2, label: 'exit threshold', min: 10, max: 400 },
		{ key: 3, label: 'debounce scans', min: 1, max: 10 },
		{ key: 6, label: 'LED brightness', min: 0, max: 255 },
		{ key: 9, label: 'orientation', min: 0, max: 7 }
	];
	let nodeTarget = $state(0);
	let nodeKey = $state(1);
	let nodeValue = $state(70);
	const nodeSpec = $derived(NODE_KEYS.find((k) => k.key === nodeKey) ?? NODE_KEYS[0]);

	const modes: DetectMode[] = ['auto', 'suggest', 'off'];
	const tun = $derived(game.tunables);

	// The rail renders itself from the schema the server ships, so it can never
	// offer a value the server would clamp, adding a knob is one line of Rust,
	// and the defaults stop existing in triplicate.
	const groups = $derived.by(() => {
		// Scratch, not state: built fresh on every run and returned as an array,
		// so there is nothing here for a SvelteMap to make reactive.
		// eslint-disable-next-line svelte/prefer-svelte-reactivity
		const out = new Map<string, Setting[]>();
		for (const spec of game.settings) {
			const list = out.get(spec.group) ?? [];
			list.push(spec);
			out.set(spec.group, list);
		}
		return [...out];
	});

	// A slider bound straight to the broadcast value fights the operator's thumb:
	// during setup the server is dirty on every tick, so each incoming state
	// re-applies its own value to the DOM node mid-drag. Track locally while
	// dragging, and commit on release.
	let dragging = $state<Record<string, number>>({});

	function shown(spec: Setting): number {
		const live = dragging[spec.key];
		if (live !== undefined) return live;
		return Number(tun[spec.key] ?? spec.min);
	}

	function format(spec: Setting, value: number): string {
		const text = spec.kind === 'float' ? value.toFixed(2).replace(/\.?0+$/, '') : String(value);
		return `${text}${spec.unit}`;
	}

	function tune(key: string, value: number) {
		send('set_config', { key, value });
	}

	function commit(spec: Setting, value: number) {
		delete dragging[spec.key];
		dragging = { ...dragging };
		tune(spec.key, value);
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

				{#each groups as [group, specs] (group)}
					<p class="grouphead">{group}</p>
					{#each specs as spec (spec.key)}
						{#if spec.kind === 'bool'}
							<label class="toggle" title={spec.help}>
								<input
									type="checkbox"
									checked={Boolean(tun[spec.key])}
									onchange={(e) =>
										send('set_config', { key: spec.key, value: e.currentTarget.checked })}
								/>
								<span
									>{spec.label}{#if !spec.live}<em class="next"> next game</em>{/if}</span
								>
							</label>
						{:else}
							<label class="slider" title={spec.help}>
								<span>
									{spec.label}
									<b class="tnum">{format(spec, shown(spec))}</b>
									{#if !spec.live}<em class="next">next game</em>{/if}
								</span>
								<input
									type="range"
									min={spec.min}
									max={spec.max}
									step={spec.step}
									value={shown(spec)}
									oninput={(e) =>
										(dragging = { ...dragging, [spec.key]: Number(e.currentTarget.value) })}
									onchange={(e) => commit(spec, Number(e.currentTarget.value))}
								/>
							</label>
						{/if}
					{/each}
				{/each}
			</section>

			<section class="card">
				<div class="cardhead">
					<h3>Quadrant hardware</h3>
					<span class="stat">EEPROM</span>
				</div>
				<p class="hint">
					Written straight to the AVR. Each commit costs ~240 ms of EEPROM, so change one at a time.
				</p>
				<div class="row">
					{#each [0, 1, 2, 3] as node (node)}
						<button
							class="chip"
							class:active={nodeTarget === node}
							onclick={() => (nodeTarget = node)}
						>
							node {node}
						</button>
					{/each}
				</div>
				<div class="row">
					<select bind:value={nodeKey} class="grow">
						{#each NODE_KEYS as k (k.key)}
							<option value={k.key}>{k.label}</option>
						{/each}
					</select>
					<input
						type="number"
						class="num"
						min={nodeSpec.min}
						max={nodeSpec.max}
						bind:value={nodeValue}
					/>
					<button
						class="chip"
						onclick={() =>
							send('node_config', {
								node: nodeTarget,
								key: nodeKey,
								value: Math.max(nodeSpec.min, Math.min(nodeSpec.max, nodeValue))
							})}>write</button
					>
				</div>
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
		width: min(320px, 100%);
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
	.grouphead {
		margin: 12px 0 2px;
		font-family: var(--font-mono);
		font-size: 10px;
		letter-spacing: 0.14em;
		text-transform: uppercase;
		color: var(--color-fg-faint);
	}
	.grouphead:first-of-type {
		margin-top: 4px;
	}
	.next {
		font-style: normal;
		font-size: 9px;
		padding: 1px 5px;
		margin-left: 6px;
		border-radius: 999px;
		border: 1px solid var(--color-line);
		color: var(--color-fg-faint);
	}
	.toggle {
		display: flex;
		align-items: center;
		gap: 8px;
		margin: 6px 0;
		font-size: 12px;
		color: var(--color-fg-dim);
		cursor: pointer;
	}
	.slider b {
		float: right;
		color: var(--color-fg);
		font-weight: 500;
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

	/* Stacked under the board rather than beside it: full width, and scrolling
	   with the page instead of inside a box the thumb has to find first. The
	   header sticks so `hide` is always one reach away — this rail is long, and
	   burying the way to close it is what makes a phone operator scroll past
	   the board to get back to the game. */
	@media (max-width: 1180px) {
		.rail {
			width: 100%;
			max-height: none;
			overflow-y: visible;
		}
		.railhead {
			position: sticky;
			top: 0;
			z-index: 5;
			padding: 6px 0;
			margin: -6px 0 0;
			background: linear-gradient(var(--color-void) 70%, transparent);
		}
	}

	@media (max-width: 700px) {
		/* Every control here is aimed at with a thumb, not a cursor. */
		.row {
			gap: 8px;
			margin-bottom: 10px;
		}
		input,
		select {
			padding: 8px 9px;
		}
		input.num {
			width: 92px;
		}
		.mini {
			gap: 6px;
		}
		.mini select {
			min-height: 34px;
		}
		.slider {
			gap: 5px;
			margin-bottom: 12px;
			font-size: 11px;
		}
		.grouphead {
			margin-top: 14px;
		}
		form {
			flex-wrap: wrap;
			gap: 8px;
		}
		form input {
			flex: 1 1 100%;
		}
	}

	/* Touch, not width: a tablet in landscape is still driven with a thumb. A
	   default checkbox is a 13px target — the label already extends the hit
	   area, but the box is where the thumb actually aims. */
	@media (pointer: coarse) {
		.toggle {
			min-height: 40px;
			margin: 0;
			font-size: 13px;
		}
		.toggle input[type='checkbox'] {
			flex: none;
			width: 22px;
			height: 22px;
			padding: 0;
		}
		.mini select {
			min-height: 34px;
		}
	}
</style>
