// Shared protocol types for the device WebSocket API (see docs/websocket-api.md)
// and the client fan-out API (see docs/client-api.md).

export type SquareState = 'empty' | 'positive' | 'negative' | 'uncertain';

export interface NodeSummary {
	node: number;
	online: boolean;
	calibrated?: boolean;
	firmware?: string;
	reset_cause?: string | number;
	timeouts?: number;
}

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
	nodes?: NodeSummary[];
	online?: boolean;
	calibrated?: boolean;
	firmware?: string;
	reset_cause?: string | number;
	rssi?: number;
	heap?: number;
	uptime?: number;
	wifi_rssi?: number;
	free_heap?: number;
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
	online_node_mask?: number;
	raw_adc?: (number | null)[];
	baseline_adc?: (number | null)[];
	noise_adc?: (number | null)[];
}

// Sensor front-end constants, taken from the firmware (10-bit ADC, AVCC ref).
//   volts = adc * measured_avcc_mv / 1023   — see docs/websocket-api.md.
// AVCC is nominally 5.0 V but measured at runtime, so raw counts stay
// authoritative and volts are an estimate. (firmware-atmega/src/bringup_config.h)
export const AVCC_MV = 5000; // nominal reference; device may report a measured value
export const ADC_MAX = 1023;
export const ADC_CENTER = 512; // kDefaultAdcMidpoint — op-amp 2.5 V mid-rail
// Conditioned op-amp output spans ~1 V–4 V (≈205–818 counts), i.e. ±~307 counts
// of signed swing from centre. This fixes the diverging colour full-scale.
export const FULL_SWING_COUNTS = 307;
// Piece-detection thresholds in ADC counts (kDefaultEnter/ExitThreshold).
export const ENTER_THRESHOLD = 70;
export const EXIT_THRESHOLD = 42;

export function adcToVolts(adc: number | null | undefined): number | null {
	if (adc == null) return null;
	return (adc * AVCC_MV) / ADC_MAX / 1000;
}

// The sensor front-end centres each square at the op-amp mid-rail; a piece pushes
// the reading above (positive polarity) or below (negative polarity) that centre.
// So the heatmap is DIVERGING: a neutral-grey midpoint (empty/baseline) diverging
// to the board's own polarity hues — green for +, slate blue for −. `t` is the
// signed, normalised deviation in [-1, +1].
const DIVERGE_STOPS: [number, [number, number, number]][] = [
	[-1, [108, 126, 205]], // strong negative — slate blue
	[-0.5, [64, 80, 120]],
	[0, [58, 64, 72]], // neutral grey midpoint (near board surface)
	[0.5, [72, 118, 92]],
	[1, [120, 190, 140]] // strong positive — sage green
];

export function polarityColor(t: number): string {
	const x = Math.max(-1, Math.min(1, t));
	let i = 0;
	while (i < DIVERGE_STOPS.length - 2 && x > DIVERGE_STOPS[i + 1][0]) i++;
	const [t0, a] = DIVERGE_STOPS[i];
	const [t1, b] = DIVERGE_STOPS[i + 1];
	const f = (x - t0) / (t1 - t0);
	const mix = (k: number) => Math.round(a[k] + (b[k] - a[k]) * f);
	return `rgb(${mix(0)}, ${mix(1)}, ${mix(2)})`;
}

// Legend gradient: negative pole → neutral → positive pole.
export const POLARITY_GRADIENT = `linear-gradient(90deg, ${[-1, -0.5, 0, 0.5, 1]
	.map((t) => polarityColor(t))
	.join(', ')})`;

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
