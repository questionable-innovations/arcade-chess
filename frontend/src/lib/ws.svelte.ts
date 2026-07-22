import {
	emptyDevice,
	summarize,
	type DeviceState,
	type Envelope,
	type EventData,
	type InMsg
} from './types';

interface TickEntry {
	id: number;
	at: string;
	text: string;
	level: 'info' | 'warn' | 'error' | 'event';
}

const BACKOFF_MIN = 1000;
const BACKOFF_MAX = 15000;
const STABLE_MS = 5000;
const LOG_MAX = 250;
const AUTH_PASSWORD_KEY_PREFIX = 'arcade-chess.admin-password:';

// Classify an envelope for log colour-coding in the debug console.
function levelOf(env: Envelope): TickEntry['level'] {
	if (env.type === 'diagnostic.log') {
		const l = (env.data?.level ?? '').toLowerCase();
		if (l === 'error' || l === 'fatal') return 'error';
		if (l === 'warn' || l === 'warning') return 'warn';
	}
	if (env.type === 'command.result' && env.status && env.status !== 'applied') return 'warn';
	if (env.type === 'calibration.result' && env.data?.ok === false) return 'error';
	return 'event';
}

function resolveUrl(): string {
	const override = new URLSearchParams(location.search).get('backend');
	if (override) return override;
	const h = location.hostname;
	if (h === 'localhost' || h === '127.0.0.1') return 'ws://localhost:8080/ws';
	return 'wss://chess-be.qinnovate.nz/ws';
}

function rebuildSquares(dev: DeviceState, d: EventData): void {
	const sq = d.squares ?? [];
	const valid = d.valid ?? [];
	for (let i = 0; i < 64; i++) {
		const v = sq[i];
		dev.squares[i] = v === 1 ? 'positive' : v === -1 ? 'negative' : 'empty';
		dev.valid[i] = valid.length ? !!valid[i] : true;
	}
}

function rebuildNodes(dev: DeviceState, env: Envelope, d: EventData): void {
	for (const summary of d.nodes ?? []) {
		if (!Number.isInteger(summary.node) || summary.node < 0 || summary.node >= 4) continue;
		dev.node_status[summary.node] = {
			v: env.v,
			type: 'node.status',
			device_id: env.device_id,
			boot_id: env.boot_id,
			seq: env.seq,
			at_ms: env.at_ms,
			data: { ...summary }
		};
	}
}

// Rebuild device state from one relayed event, honouring (boot_id, seq) continuity:
// snapshots supersede everything; a seq gap freezes sensor.changed until the next snapshot.
function applyEvent(dev: DeviceState, env: Envelope): void {
	const d = env.data ?? {};
	if (env.type === 'node.status' && typeof d.node === 'number' && d.node >= 0 && d.node < 4) {
		dev.node_status[d.node] = env;
	} else if (env.type === 'device.status') {
		dev.device_status = env;
	} else if (env.type === 'sensor.raw_scan') {
		dev.raw_scan = env;
	}

	// Seqless envelopes (e.g. command.result) carry no boot_id/seq; they update the
	// side-effects above but must never touch boot/seq/gap continuity tracking.
	if (typeof env.seq !== 'number') return;

	const boot = env.boot_id ?? null;
	const seq = env.seq;

	if (env.type === 'board.snapshot') {
		if (boot !== dev.bootId || seq >= dev.seq) {
			dev.snapshot = env;
			rebuildSquares(dev, d);
			rebuildNodes(dev, env, d);
			dev.bootId = boot;
			dev.seq = seq;
			dev.gap = false;
		}
		return;
	}

	if (boot !== dev.bootId) {
		dev.bootId = boot;
		dev.seq = seq;
		dev.gap = true;
		return;
	}

	if (seq > dev.seq) {
		const contiguous = seq === dev.seq + 1;
		dev.seq = seq;
		if (!contiguous) dev.gap = true;
		if (env.type === 'sensor.changed' && contiguous && !dev.gap) {
			if (typeof d.square === 'number' && d.square >= 0 && d.square < 64 && d.state) {
				dev.squares[d.square] = d.state;
			}
		}
	}
}

class WsStore {
	connected = $state(false);
	authed = $state(false);
	devices = $state<Record<string, DeviceState>>({});
	order = $state<string[]>([]);
	events = $state<TickEntry[]>([]);
	streaming = $state(false);

	#socket: WebSocket | null = null;
	#backoff = BACKOFF_MIN;
	#stableTimer: ReturnType<typeof setTimeout> | null = null;
	#started = false;
	#tickId = 0;
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

	auth(password: string): void {
		this.#pendingAuthPassword = password;
		this.#send({ type: 'auth', password });
	}

	// Hardware connectivity probe: light one square blue for 3s (see client-api.md).
	probe(deviceId: string, square: number): void {
		this.#send({
			type: 'command',
			device_id: deviceId,
			name: 'lighting.set',
			args: { squares: [square], effect: 'solid', colour: '00a0ff', duration_ms: 3000 }
		});
	}

	// One averaged raw voltage scan of every online square.
	rawScan(deviceId: string): void {
		this.#send({
			type: 'command',
			device_id: deviceId,
			name: 'sensor.raw_scan.get',
			args: { samples_per_square: 4 }
		});
	}

	// Toggle continuous raw voltage streaming for the live debug readout.
	setStream(deviceId: string, enabled: boolean): void {
		this.streaming = enabled;
		this.#send({
			type: 'command',
			device_id: deviceId,
			name: 'sensor.raw_stream.set',
			args: { enabled, interval_ms: 500, samples_per_square: 2 }
		});
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
			const password = this.#pendingAuthPassword ?? this.#rememberedPassword();
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
			if (this.#stableTimer) {
				clearTimeout(this.#stableTimer);
				this.#stableTimer = null;
			}
			this.#scheduleReconnect();
		};
		socket.onerror = () => socket.close();
	}

	#scheduleReconnect(): void {
		const base = this.#backoff;
		const wait = base / 2 + (Math.random() * base) / 2;
		this.#backoff = Math.min(BACKOFF_MAX, base * 2);
		setTimeout(() => this.#open(), wait);
	}

	#send(obj: unknown): void {
		if (this.#socket && this.#socket.readyState === WebSocket.OPEN) {
			this.#socket.send(JSON.stringify(obj));
		}
	}

	#onMessage(e: MessageEvent): void {
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
			case 'event':
				if (msg.device_id && msg.event) this.#handleEvent(msg.device_id, msg.event);
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
						this.#rememberPassword(this.#pendingAuthPassword);
					} else {
						this.#forgetPassword(this.#pendingAuthPassword);
					}
					this.#pendingAuthPassword = null;
				}
				this.#pushInfo(`auth ${msg.ok ? 'ok' : 'failed'}`);
				break;
			case 'command.queued':
				this.#pushInfo(`command.queued ${msg.id ?? ''}`.trim());
				break;
			case 'error':
				this.#pushInfo(`error ${msg.reason ?? ''}`.trim());
				break;
			default:
				break;
		}
	}

	#handleInit(msg: InMsg): void {
		const devices: Record<string, DeviceState> = {};
		const order: string[] = [];
		const ticks: TickEntry[] = [];
		for (const dv of msg.devices ?? []) {
			const dev = emptyDevice(dv.device_id);
			dev.connected = !!dv.connected;
			if (dv.node_status) {
				for (let n = 0; n < 4; n++) dev.node_status[n] = dv.node_status[n] ?? null;
			}
			dev.device_status = dv.device_status ?? null;
			if (dv.snapshot) applyEvent(dev, dv.snapshot);
			const recent = dv.recent ?? [];
			// Replay recent events (oldest first) so the board and seq baseline track the
			// newest event, not a stale stored snapshot. The server also requests a fresh
			// snapshot from each device on client connect; the replay bridges the window
			// until that snapshot arrives.
			for (const env of recent) applyEvent(dev, env);
			for (const env of recent.slice(-LOG_MAX)) ticks.push(this.#makeTick(env));
			if (recent.length) dev.lastEventAt = Date.now();
			devices[dv.device_id] = dev;
			order.push(dv.device_id);
		}
		this.devices = devices;
		this.order = order;
		this.events = ticks.reverse().slice(0, LOG_MAX);
	}

	#handleEvent(deviceId: string, env: Envelope): void {
		const dev = this.#ensure(deviceId);
		applyEvent(dev, env);
		dev.lastEventAt = Date.now();
		this.events = [this.#makeTick(env), ...this.events].slice(0, LOG_MAX);
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

	#makeTick(env: Envelope): TickEntry {
		return {
			id: this.#tickId++,
			at: env.at_ms != null ? String(env.at_ms) : '·',
			text: summarize(env),
			level: levelOf(env)
		};
	}

	#pushInfo(text: string, level: TickEntry['level'] = 'info'): void {
		this.events = [{ id: this.#tickId++, at: '·', text, level }, ...this.events].slice(0, LOG_MAX);
	}

	#authPasswordKey(): string {
		return `${AUTH_PASSWORD_KEY_PREFIX}${resolveUrl()}`;
	}

	#rememberedPassword(): string | null {
		try {
			return localStorage.getItem(this.#authPasswordKey());
		} catch {
			return null;
		}
	}

	#rememberPassword(password: string): void {
		try {
			localStorage.setItem(this.#authPasswordKey(), password);
		} catch {
			// Authentication still works when storage is unavailable.
		}
	}

	#forgetPassword(password: string): void {
		try {
			const key = this.#authPasswordKey();
			if (localStorage.getItem(key) === password) localStorage.removeItem(key);
		} catch {
			// Nothing to clear when storage is unavailable.
		}
	}

	// ── Offline demo harness ──────────────────────────────────────────────────
	// Builds a plausible full-board state so the interface can be reviewed and
	// screenshotted without a live device. Activated by the `?demo` query flag.
	#loadDemo(): void {
		this.connected = true;
		const dev = emptyDevice('arcade-chess-001');
		dev.connected = true;
		dev.bootId = '7e4c18b2';
		dev.seq = 412;
		dev.lastEventAt = Date.now();

		// Standard opening position: back two ranks each side carry pieces.
		const pos = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
		const neg = [48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63];
		for (const i of pos) dev.squares[i] = 'positive';
		for (const i of neg) dev.squares[i] = 'negative';
		dev.squares[27] = 'uncertain';
		dev.valid[42] = false;

		for (let n = 0; n < 4; n++) {
			dev.node_status[n] = {
				type: 'node.status',
				seq: 100 + n,
				at_ms: 60000,
				data: { node: n, online: true, calibrated: n !== 2, firmware: '0.1.0' }
			};
		}
		dev.device_status = {
			type: 'device.status',
			data: { rssi: -58, heap: 184320, uptime: 4210 }
		};
		// Op-amp centres each square at ~512 (2.5 V mid-rail); a piece pushes the
		// reading ±~285 counts by polarity. Empty squares hover in the deadband.
		dev.raw_scan = {
			type: 'sensor.raw_scan',
			data: {
				scan_id: 7,
				complete: true,
				response_node_mask: 0b1111,
				baseline_adc: Array.from({ length: 64 }, () => 512),
				noise_adc: Array.from({ length: 64 }, (_, i) => 3 + ((i * 7) % 4)),
				raw_adc: Array.from({ length: 64 }, (_, i) => {
					// Deterministic pseudo-noise so the readout looks alive but stable.
					const jitter = ((i * 37) % 23) - 11;
					const s = dev.squares[i];
					if (s === 'positive') return 512 + 285 + jitter;
					if (s === 'negative') return 512 - 285 + jitter;
					if (s === 'uncertain') return 512 + 58 + jitter; // near the enter threshold
					return 512 + Math.round(jitter / 3); // empty: inside the deadband
				})
			}
		};

		this.devices = { 'arcade-chess-001': dev };
		this.order = ['arcade-chess-001'];
		this.events = [
			{ type: 'device.status', data: { rssi: -58, heap: 184320, uptime: 4210 }, at_ms: 60000 },
			{ type: 'node.status', data: { node: 2, online: true, calibrated: false }, at_ms: 59120 },
			{
				type: 'diagnostic.log',
				data: { level: 'warn', component: 'node2', message: 'awaiting calibration' },
				at_ms: 58900
			},
			{
				type: 'sensor.changed',
				seq: 412,
				data: { square: 27, state: 'uncertain', raw: 471 },
				at_ms: 58400
			},
			{
				type: 'sensor.raw_scan',
				data: { scan_id: 7, complete: true, response_node_mask: 15 },
				at_ms: 57800
			},
			{
				type: 'sensor.changed',
				seq: 411,
				data: { square: 12, state: 'positive', raw: 702 },
				at_ms: 57200
			},
			{ type: 'board.snapshot', seq: 410, data: { valid: dev.valid }, at_ms: 56000 }
		].map((e) => this.#makeTick(e as Envelope));
	}
}

export const ws = new WsStore();
