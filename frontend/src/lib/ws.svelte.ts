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
}

const BACKOFF_MIN = 1000;
const BACKOFF_MAX = 15000;
const STABLE_MS = 5000;

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

// Rebuild device state from one relayed event, honouring (boot_id, seq) continuity:
// snapshots supersede everything; a seq gap freezes sensor.changed until the next snapshot.
function applyEvent(dev: DeviceState, env: Envelope): void {
	const d = env.data ?? {};
	if (env.type === 'node.status' && typeof d.node === 'number' && d.node >= 0 && d.node < 4) {
		dev.node_status[d.node] = env;
	} else if (env.type === 'device.status') {
		dev.device_status = env;
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

	#socket: WebSocket | null = null;
	#backoff = BACKOFF_MIN;
	#stableTimer: ReturnType<typeof setTimeout> | null = null;
	#started = false;
	#tickId = 0;

	connect(): void {
		if (this.#started) return;
		this.#started = true;
		this.#open();
	}

	auth(password: string): void {
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
			// newest event, not a stale stored snapshot; the server does not request a
			// fresh snapshot on browser connect, so otherwise the first live sensor.changed
			// reads as a gap and freezes updates with no recovery path.
			for (const env of recent) applyEvent(dev, env);
			for (const env of recent.slice(-8)) ticks.push(this.#makeTick(env));
			if (recent.length) dev.lastEventAt = Date.now();
			devices[dv.device_id] = dev;
			order.push(dv.device_id);
		}
		this.devices = devices;
		this.order = order;
		this.events = ticks.reverse().slice(0, 8);
	}

	#handleEvent(deviceId: string, env: Envelope): void {
		const dev = this.#ensure(deviceId);
		applyEvent(dev, env);
		dev.lastEventAt = Date.now();
		this.events = [this.#makeTick(env), ...this.events].slice(0, 8);
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
			text: summarize(env)
		};
	}

	#pushInfo(text: string): void {
		this.events = [{ id: this.#tickId++, at: '·', text }, ...this.events].slice(0, 8);
	}
}

export const ws = new WsStore();
