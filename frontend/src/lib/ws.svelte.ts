import { forgetPassword, rememberedPassword, rememberPassword, resolveUrl } from './backend';
import { demoDevice, demoEvents } from './demo';
import { applyEvent } from './reducer';
import { TickLog, type BusFrame } from './ticklog.svelte';
import { WaveRunner } from './wave.svelte';
import { IDLE_GAME, type GameState } from './game/types';
import {
	emptyDevice,
	NODE_COUNT,
	SERVER_ERRORS,
	type DeviceState,
	type Envelope,
	type InMsg,
	type TickEntry
} from './types';

const BACKOFF_MIN = 1000;
const BACKOFF_MAX = 15000;
const STABLE_MS = 5000;
// A half-open socket never fires onclose, so nothing but inbound traffic proves
// the link. Browsers answer the server's 15 s ping transparently and a pong never
// reaches onmessage, so only real frames count. The server drops a silent device
// after 45 s and tells us, so 60 s guarantees that verdict lands first and a dead
// board is never mistaken for a dead browser link.
const LIVENESS_CHECK_MS = 10000;
const LIVENESS_TIMEOUT_MS = 60000;

// Older firmware omits the flag entirely; null means "not reported", not "off".
function boolOrNull(v: boolean | undefined): boolean | null {
	return typeof v === 'boolean' ? v : null;
}

class WsStore {
	connected = $state(false);
	authed = $state(false);
	devices = $state<Record<string, DeviceState>>({});
	order = $state<string[]>([]);
	// Puzzle mode's whole state, replaced wholesale on every `game.state`.
	// Snapshots supersede: no incremental sync, no drift.
	game = $state<GameState>(IDLE_GAME);
	#log = new TickLog();

	get events(): TickEntry[] {
		return this.#log.entries;
	}

	get busFrames(): BusFrame[] {
		return this.#log.busFrames;
	}

	// What we asked for vs what the device last reported (device.status). A toggle
	// clears the report so the button answers instantly, then the device's own
	// truth takes over — an optimistic flag alone outlives the stream it claims.
	#streamWanted = $state(false);
	#traceWanted = $state(false);
	#streamReported = $state<boolean | null>(null);
	#traceReported = $state<boolean | null>(null);
	#wave = new WaveRunner({
		send: (obj) => this.#send(obj),
		notify: (text, level) => this.#pushInfo(text, level)
	});

	get streaming(): boolean {
		return this.#streamReported ?? this.#streamWanted;
	}

	get waving(): boolean {
		return this.#wave.active;
	}

	get tracing(): boolean {
		return this.#traceReported ?? this.#traceWanted;
	}

	#socket: WebSocket | null = null;
	#backoff = BACKOFF_MIN;
	#stableTimer: ReturnType<typeof setTimeout> | null = null;
	#reconnectTimer: ReturnType<typeof setTimeout> | null = null;
	#liveTimer: ReturnType<typeof setInterval> | null = null;
	#lastMsgAt = 0;
	#started = false;
	#pendingAuthPassword: string | null = null;

	connect(): void {
		if (this.#started) return;
		this.#started = true;
		// Offline design/QA harness — populate a realistic board without hardware.
		if (typeof location !== 'undefined' && new URLSearchParams(location.search).has('demo')) {
			this.#loadDemo();
			return;
		}
		this.#open();
	}

	// Release every timer and the socket; connect() may be called again after.
	teardown(): void {
		this.#started = false;
		this.#stopLocalActivity();
		// Only a deliberate teardown cancels a pending reconnect; a dropped link
		// leaves it running so the client comes back on its own.
		if (this.#reconnectTimer) {
			clearTimeout(this.#reconnectTimer);
			this.#reconnectTimer = null;
		}
		const socket = this.#socket;
		this.#socket = null;
		if (socket) {
			socket.onclose = null;
			socket.onerror = null;
			// A frame still in flight would otherwise repopulate a discarded store.
			socket.onmessage = null;
			socket.close();
		}
		this.connected = false;
	}

	auth(password: string): void {
		this.#pendingAuthPassword = password;
		this.#send({ type: 'auth', password });
	}

	// Every puzzle-mode action goes through here. A success is acknowledged by
	// the next `game.state`; a rejection arrives as the usual `error` envelope.
	sendGame(action: string, extra: Record<string, unknown> = {}): boolean {
		return this.#send({ type: 'game', action, ...extra });
	}

	// Hardware connectivity probe: light one square blue for 3s (see client-api.md).
	probe(deviceId: string, square: number): boolean {
		return this.#send({
			type: 'command',
			device_id: deviceId,
			name: 'lighting.set',
			args: { squares: [square], effect: 'solid', colour: '00a0ff', duration_ms: 3000 }
		});
	}

	// One averaged raw voltage scan of every online square.
	rawScan(deviceId: string): boolean {
		return this.#send({
			type: 'command',
			device_id: deviceId,
			name: 'sensor.raw_scan.get',
			args: { samples_per_square: 4 }
		});
	}

	// Start baseline calibration on one node (0-3) or every online node.
	calibrate(deviceId: string, node: number | 'all'): boolean {
		return this.#send({
			type: 'command',
			device_id: deviceId,
			name: 'calibration.start',
			args: { node }
		});
	}

	// Toggle the UART frame trace (diagnostic.bus events with hex payloads).
	setTrace(deviceId: string, enabled: boolean): void {
		const sent = this.#send({
			type: 'command',
			device_id: deviceId,
			name: 'diagnostics.trace',
			args: { enabled, raw_frames: true, duration_ms: 300000 }
		});
		if (!sent) return;
		this.#traceWanted = enabled;
		this.#traceReported = null;
		if (enabled) this.#log.clearBus();
	}

	wave(deviceId: string): void {
		this.#wave.start(deviceId);
	}

	stopWave(deviceId: string): void {
		this.#wave.stop(deviceId);
	}

	// Toggle continuous raw voltage streaming for the live debug readout.
	setStream(deviceId: string, enabled: boolean): void {
		const sent = this.#send({
			type: 'command',
			device_id: deviceId,
			name: 'sensor.raw_stream.set',
			args: { enabled, interval_ms: 500, samples_per_square: 2 }
		});
		if (!sent) return;
		this.#streamWanted = enabled;
		this.#streamReported = null;
	}

	#open(): void {
		let socket: WebSocket;
		try {
			socket = new WebSocket(resolveUrl());
		} catch {
			this.#scheduleReconnect();
			return;
		}
		this.#socket = socket;
		socket.onopen = () => {
			this.connected = true;
			this.authed = false;
			this.#lastMsgAt = Date.now();
			this.#startWatchdog();
			const password = this.#pendingAuthPassword ?? rememberedPassword();
			if (password) {
				this.#pendingAuthPassword = password;
				this.#send({ type: 'auth', password });
			}
			// Reset backoff only once the connection proves stable, so a server that
			// accepts then immediately drops keeps backing off instead of hot-looping.
			this.#stableTimer = setTimeout(() => {
				this.#backoff = BACKOFF_MIN;
				this.#stableTimer = null;
			}, STABLE_MS);
		};
		socket.onmessage = (e) => this.#onMessage(e);
		socket.onclose = () => {
			this.connected = false;
			// The server re-sends the truth in `init`; until then nothing is live, so
			// the admin rail must not keep offering commands that silently vanish.
			this.authed = false;
			for (const dev of Object.values(this.devices)) dev.connected = false;
			this.#streamWanted = false;
			this.#traceWanted = false;
			this.#streamReported = null;
			this.#traceReported = null;
			this.#stopLocalActivity();
			this.#scheduleReconnect();
		};
		socket.onerror = () => socket.close();
	}

	// Everything this client drives on its own, stopped without touching the socket
	// or a pending reconnect.
	#stopLocalActivity(): void {
		this.#wave.cancel();
		if (this.#stableTimer) {
			clearTimeout(this.#stableTimer);
			this.#stableTimer = null;
		}
		if (this.#liveTimer) {
			clearInterval(this.#liveTimer);
			this.#liveTimer = null;
		}
	}

	// Force a close on a socket that has gone quiet; onclose then drives reconnect.
	// Only a connected device produces the periodic traffic this reads as proof of
	// life, so with no board attached silence is normal and the deadline must keep
	// sliding — otherwise a browser left open on an empty rack reconnects forever.
	#startWatchdog(): void {
		if (this.#liveTimer) clearInterval(this.#liveTimer);
		this.#liveTimer = setInterval(() => {
			// The server emits a `keepalive` text frame every 15 s whether or not a
			// board is attached, so silence past the timeout means the path is gone.
			// A WebSocket ping cannot stand in for it: the browser answers pings in
			// the transport and never surfaces them here.
			if (Date.now() - this.#lastMsgAt <= LIVENESS_TIMEOUT_MS) return;
			this.#pushInfo('link silent — reconnecting', 'warn');
			this.#socket?.close();
		}, LIVENESS_CHECK_MS);
	}

	#scheduleReconnect(): void {
		if (!this.#started) return;
		const base = this.#backoff;
		const wait = base / 2 + (Math.random() * base) / 2;
		this.#backoff = Math.min(BACKOFF_MAX, base * 2);
		this.#reconnectTimer = setTimeout(() => {
			this.#reconnectTimer = null;
			this.#open();
		}, wait);
	}

	#send(obj: unknown): boolean {
		if (!this.#socket || this.#socket.readyState !== WebSocket.OPEN) {
			this.#pushInfo('send dropped: link down', 'error');
			return false;
		}
		this.#socket.send(JSON.stringify(obj));
		return true;
	}

	#onMessage(e: MessageEvent): void {
		this.#lastMsgAt = Date.now();
		let msg: InMsg;
		try {
			msg = JSON.parse(e.data as string) as InMsg;
		} catch {
			return;
		}
		if (!msg || typeof msg.type !== 'string') return;

		switch (msg.type) {
			case 'init':
				this.#handleInit(msg);
				break;
			case 'game.state':
				this.game = msg as unknown as GameState;
				break;
			case 'event':
				if (msg.device_id && msg.event)
					this.#handleEvent(msg.device_id, msg.event, msg.recv_unix_ms);
				break;
			case 'device.connected':
				if (msg.device_id) this.#setConnected(msg.device_id, true);
				break;
			case 'device.disconnected':
				if (msg.device_id) this.#setConnected(msg.device_id, false);
				break;
			case 'auth.result':
				this.authed = !!msg.ok;
				if (this.#pendingAuthPassword) {
					if (msg.ok) {
						rememberPassword(this.#pendingAuthPassword);
					} else {
						forgetPassword(this.#pendingAuthPassword);
					}
					this.#pendingAuthPassword = null;
				}
				this.#pushInfo(`auth ${msg.ok ? 'ok' : 'failed'}`);
				break;
			case 'command.queued':
				this.#pushInfo(`command.queued ${msg.id ?? ''}`.trim());
				break;
			case 'error': {
				const known = SERVER_ERRORS[msg.reason ?? ''];
				this.#pushInfo(known?.text ?? `error ${msg.reason ?? ''}`.trim(), known?.level ?? 'warn');
				break;
			}
			default:
				break;
		}
	}

	#handleInit(msg: InMsg): void {
		// The game rides along in `init`, so a browser refresh mid-demo is free.
		if (msg.game && typeof msg.game.phase === 'string') this.game = msg.game;
		const devices: Record<string, DeviceState> = {};
		const order: string[] = [];
		const ticks: Envelope[] = [];
		const busReplay: Envelope[] = [];
		for (const dv of msg.devices ?? []) {
			const dev = emptyDevice(dv.device_id);
			dev.connected = !!dv.connected;
			if (dv.node_status) {
				for (let n = 0; n < NODE_COUNT; n++) dev.node_status[n] = dv.node_status[n] ?? null;
			}
			dev.device_status = dv.device_status ?? null;
			if (dv.snapshot) applyEvent(dev, dv.snapshot);
			const recent = dv.recent ?? [];
			// Replay recent events (oldest first) so the board and seq baseline track the
			// newest event, not a stale stored snapshot. The server also requests a fresh
			// snapshot from each device on client connect; the replay bridges the window
			// until that snapshot arrives.
			for (const env of recent) applyEvent(dev, env);
			// An active trace fills the server's recent ring with bus frames; route
			// them to the bus tab so they don't evict every semantic console entry.
			for (const env of recent) {
				if (env.type === 'diagnostic.bus') busReplay.push(env);
				else ticks.push(env);
			}
			// lastEventAt stays 0 here: replayed history is not liveness, and stamping
			// it makes a board that died an hour ago read as "live" after a reload.
			devices[dv.device_id] = dev;
			order.push(dv.device_id);
		}
		this.devices = devices;
		this.order = order;
		// Re-seed the stream/trace truth from whichever board reported last.
		const reported = order.map((id) => devices[id].device_status?.data).find((d) => d != null);
		this.#streamReported = boolOrNull(reported?.raw_stream);
		this.#traceReported = boolOrNull(reported?.trace);
		this.#log.seed(ticks, busReplay, '── reconnected · entries below predate the drop ──');
	}

	#handleEvent(deviceId: string, env: Envelope, recvUnixMs?: number): void {
		const dev = this.#ensure(deviceId);
		applyEvent(dev, env);
		dev.lastEventAt = recvUnixMs ?? Date.now();
		if (env.type === 'device.status') {
			this.#streamReported = boolOrNull(env.data?.raw_stream);
			this.#traceReported = boolOrNull(env.data?.trace);
		}
		if (env.type === 'diagnostic.bus') {
			this.#log.pushBus(env);
			return;
		}
		this.#log.push(env);
	}

	#setConnected(deviceId: string, value: boolean): void {
		this.#ensure(deviceId).connected = value;
	}

	#ensure(deviceId: string): DeviceState {
		let dev = this.devices[deviceId];
		if (!dev) {
			dev = emptyDevice(deviceId);
			this.devices[deviceId] = dev;
			this.order.push(deviceId);
		}
		return dev;
	}

	#pushInfo(text: string, level: TickEntry['level'] = 'info'): void {
		this.#log.pushInfo(text, level);
	}

	#loadDemo(): void {
		this.connected = true;
		const dev = demoDevice();
		this.devices = { [dev.device_id]: dev };
		this.order = [dev.device_id];
		this.#log.reset(demoEvents(dev));
	}
}

export const ws = new WsStore();
