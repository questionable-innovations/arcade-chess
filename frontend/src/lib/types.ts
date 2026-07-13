// Shared protocol types for the device WebSocket API (see docs/websocket-api.md)
// and the client fan-out API (see docs/client-api.md).

export type SquareState = 'empty' | 'positive' | 'negative' | 'uncertain';

// Loose superset of every device-event `data` payload we read.
export interface EventData {
	square?: number;
	state?: SquareState;
	raw?: number;
	baseline?: number;
	node?: number;
	local_square?: number;
	squares?: number[];
	valid?: boolean[];
	online?: boolean;
	calibrated?: boolean;
	firmware?: string;
	reset_cause?: string;
	rssi?: number;
	heap?: number;
	uptime?: number;
	level?: string;
	component?: string;
	message?: string;
	phase?: string;
	percent?: number;
	samples?: number;
	ok?: boolean;
	scan_id?: number | string;
	complete?: boolean;
	response_node_mask?: number;
	raw_adc?: (number | null)[];
}

// A device event envelope, relayed verbatim by the server.
export interface Envelope {
	v?: number;
	type: string;
	device_id?: string;
	boot_id?: string;
	seq?: number;
	at_ms?: number;
	data?: EventData;
	// command.result carries these at the top level rather than in `data`.
	id?: string;
	status?: string;
	reason?: string | null;
}

// One device as delivered inside an `init` message.
export interface DeviceView {
	device_id: string;
	connected: boolean;
	hello?: Envelope | null;
	snapshot?: Envelope | null;
	node_status?: (Envelope | null)[];
	device_status?: Envelope | null;
	recent?: Envelope[];
}

// A message from the server on the client channel.
export interface InMsg {
	type: string;
	devices?: DeviceView[];
	device_id?: string;
	event?: Envelope;
	ok?: boolean;
	id?: string;
	reason?: string;
}

// Reconstructed per-device state driving the UI.
export interface DeviceState {
	device_id: string;
	connected: boolean;
	bootId: string | null;
	seq: number;
	gap: boolean;
	squares: SquareState[];
	valid: boolean[];
	snapshot: Envelope | null;
	node_status: (Envelope | null)[];
	device_status: Envelope | null;
	raw_scan: Envelope | null;
	lastEventAt: number;
}

export type NodeHealth = 'healthy' | 'uncalibrated' | 'offline' | 'unseen';

export function nodeHealth(env: Envelope | null): NodeHealth {
	if (!env || !env.data) return 'unseen';
	if (!env.data.online) return 'offline';
	return env.data.calibrated ? 'healthy' : 'uncalibrated';
}

export function emptyDevice(id: string): DeviceState {
	return {
		device_id: id,
		connected: false,
		bootId: null,
		seq: -1,
		gap: false,
		squares: Array.from({ length: 64 }, () => 'empty' as SquareState),
		valid: Array.from({ length: 64 }, () => true),
		snapshot: null,
		node_status: [null, null, null, null],
		device_status: null,
		raw_scan: null,
		lastEventAt: 0
	};
}

// One-glance ticker summary of a relayed event.
export function summarize(env: Envelope): string {
	const d = env.data ?? {};
	switch (env.type) {
		case 'sensor.changed':
			return `sensor.changed sq${d.square} ${d.state}${d.raw != null ? ` raw=${d.raw}` : ''}`;
		case 'board.snapshot': {
			const n = d.valid ? d.valid.filter(Boolean).length : 64;
			return `board.snapshot valid=${n}/64`;
		}
		case 'sensor.raw_scan':
			return `sensor.raw_scan ${d.scan_id ?? '?'} ${d.complete ? 'complete' : 'partial'} mask=${d.response_node_mask ?? '?'}`;
		case 'node.status':
			return `node.status n${d.node} ${d.online ? 'online' : 'offline'} cal=${d.calibrated ? 1 : 0}`;
		case 'device.status':
			return `device.status rssi=${d.rssi ?? '?'} heap=${d.heap ?? '?'}`;
		case 'command.result':
			return `command.result ${env.id ?? ''} ${env.status ?? ''}${env.reason ? ` ${env.reason}` : ''}`.trim();
		case 'diagnostic.log':
			return `diagnostic.log ${d.level ?? ''} ${d.component ?? ''} ${d.message ?? ''}`.trim();
		case 'calibration.progress':
			return `calibration.progress n${d.node} ${d.phase ?? ''} ${d.percent ?? ''}%`;
		case 'calibration.result':
			return `calibration.result n${d.node} ${d.ok ? 'ok' : 'fail'}`;
		default:
			return env.type;
	}
}
