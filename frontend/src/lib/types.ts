// Shared protocol types for the device WebSocket API (see docs/websocket-api.md)
// and the client fan-out API (see docs/client-api.md).

export type SquareState = 'empty' | 'positive' | 'negative' | 'uncertain';

// Board geometry: four quadrant nodes, each owning a 4x4 block of the 8x8 board.
export const NODE_COUNT = 4;
export const SQUARE_COUNT = 64;
// Ready-made [0..NODE_COUNT) for the components that render one row per node.
export const NODE_INDICES = Array.from({ length: NODE_COUNT }, (_, i) => i);

export interface NodeSummary {
	node: number;
	online: boolean;
	calibrated?: boolean;
	firmware?: string;
	reset_cause?: string | number;
	timeouts?: number;
	consecutive_timeouts?: number;
	last_seen_ms?: number;
	// Node-reported health, only present when the node runs extended firmware
	// (STATUS payload >= 17 bytes). node_rx_bad and node_event_overflow are the
	// two that mean actual data loss.
	node_uptime_ms?: number;
	event_depth?: number;
	last_scan_ms?: number;
	node_rx_good?: number;
	node_rx_bad?: number;
	node_event_overflow?: number;
	supply_mv?: number;
	// ESP-side: times this node's uptime went backwards.
	reboots?: number;
}

// Loose superset of every device-event `data` payload we read.
export interface EventData extends Partial<NodeSummary> {
	square?: number;
	state?: SquareState;
	raw?: number;
	baseline?: number;
	local_square?: number;
	squares?: number[];
	valid?: boolean[];
	nodes?: NodeSummary[];
	// device.status
	rssi?: number;
	heap?: number;
	uptime?: number; // legacy millis field, superseded by uptime_ms
	uptime_ms?: number;
	websocket_reconnects?: number;
	uart_good?: number;
	uart_bad?: number;
	uart_timeouts?: number;
	quadrant_mask?: number;
	quadrant_count?: number;
	mode?: string;
	ws_send_failed?: number;
	events_dropped_offline?: number;
	snapshot_repairs?: number;
	raw_stream?: boolean;
	trace?: boolean;
	reset_reason?: number;
	level?: string;
	component?: string;
	message?: string;
	suppressed?: number;
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
	// diagnostic.bus
	direction?: string;
	uart_seq?: number;
	message_type?: number;
	result?: string;
	raw_hex?: string;
	dropped?: number;
	reason?: string;
	// command.result reason=node_error detail
	code?: number;
}

// UART message-type names (protocol/include/arcade_protocol/protocol.h).
export const MESSAGE_TYPE_NAMES: Record<number, string> = {
	0x01: 'PING',
	0x02: 'INFO',
	0x03: 'STATUS',
	0x04: 'TIME_SYNC',
	0x05: 'CONFIG_GET',
	0x06: 'CONFIG_SET',
	0x0f: 'ERROR',
	0x20: 'POLL_EVENTS',
	0x21: 'EVENT_BATCH',
	0x22: 'GET_SNAPSHOT',
	0x23: 'SENSOR_SNAPSHOT',
	0x24: 'GET_RAW_SCAN',
	0x25: 'RAW_SCAN',
	0x30: 'CALIBRATE',
	0x31: 'CALIBRATION_RESULT',
	0x40: 'SET_SQUARES',
	0x41: 'SET_BRIGHTNESS',
	0x42: 'IDENTIFY',
	0x43: 'CLEAR_LIGHTING',
	0x44: 'RENDER_WINDOW',
	0x50: 'SET_DEBUG',
	0x60: 'FW_PREFLIGHT',
	0x61: 'MAINTENANCE_BEGIN',
	0x62: 'FW_PREPARE',
	0x63: 'FW_ENTER_BOOTLOADER',
	0x64: 'MAINTENANCE_END',
	0x65: 'FW_HEALTH',
	0x66: 'FW_CONFIRM'
};

export function messageTypeLabel(type: number | undefined): string {
	if (type == null) return '?';
	const hex = `0x${type.toString(16).padStart(2, '0')}`;
	const name = MESSAGE_TYPE_NAMES[type];
	return name ? `${name} ${hex}` : hex;
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

// A node coming or going, as journalled by the server (newest last, max 64).
export interface NodeTransition {
	unix_ms: number;
	node: number;
	online: boolean;
	reset_cause?: number;
	timeouts?: number;
	event_overflow?: number;
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
	node_events?: NodeTransition[];
}

// A message from the server on the client channel.
export interface InMsg {
	type: string;
	devices?: DeviceView[];
	// Puzzle-mode snapshot, embedded in `init` and broadcast as `game.state`.
	game?: import('./game/types').GameState;
	device_id?: string;
	event?: Envelope;
	// Server wall-clock stamp for a relayed event, outside the device envelope.
	recv_unix_ms?: number;
	ok?: boolean;
	id?: string;
	reason?: string;
}

// Live calibration readout per node, driven by calibration.progress/result.
export interface CalibrationState {
	active: boolean;
	percent: number;
	ok?: boolean;
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
	calibration: (CalibrationState | null)[];
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
		squares: Array.from({ length: SQUARE_COUNT }, () => 'empty' as SquareState),
		valid: Array.from({ length: SQUARE_COUNT }, () => true),
		snapshot: null,
		node_status: Array.from({ length: NODE_COUNT }, () => null),
		device_status: null,
		raw_scan: null,
		calibration: Array.from({ length: NODE_COUNT }, () => null),
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
			const n = d.valid ? d.valid.filter(Boolean).length : SQUARE_COUNT;
			return `board.snapshot valid=${n}/${SQUARE_COUNT}`;
		}
		case 'sensor.raw_scan':
			return `sensor.raw_scan ${d.scan_id ?? '?'} ${d.complete ? 'complete' : 'partial'} mask=${d.response_node_mask ?? '?'}`;
		case 'node.status': {
			// Loss counters ride along in the ticker: a quadrant silently dropping
			// events should be readable without opening the rail.
			const loss = [
				d.node_rx_bad ? ` rxbad=${d.node_rx_bad}` : '',
				d.node_event_overflow ? ` ovf=${d.node_event_overflow}` : '',
				d.reboots ? ` reboots=${d.reboots}` : ''
			].join('');
			return `node.status n${d.node} ${d.online ? 'online' : 'offline'} cal=${d.calibrated ? 1 : 0}${loss}`;
		}
		case 'device.status':
			return `device.status rssi=${d.rssi ?? '?'} heap=${d.heap ?? '?'}`;
		case 'command.result': {
			const detail =
				env.reason === 'node_error' && d.code != null ? ` n${d.node} code=${d.code}` : '';
			return `command.result ${env.id ?? ''} ${env.status ?? ''}${env.reason ? ` ${env.reason}` : ''}${detail}`.trim();
		}
		case 'diagnostic.log': {
			const sup = d.suppressed ? ` (+${d.suppressed} suppressed)` : '';
			const node = d.node != null ? ` n${d.node}` : '';
			return `diagnostic.log ${d.level ?? ''} ${d.component ?? ''}${node} ${d.message ?? ''}${sup}`.trim();
		}
		case 'calibration.progress':
			return `calibration.progress n${d.node} ${d.phase ?? ''} ${d.percent ?? ''}%`;
		case 'calibration.result':
			return `calibration.result n${d.node} ${d.ok ? 'ok' : 'fail'}${d.reason ? ` ${d.reason}` : ''}`;
		case 'diagnostic.bus':
			return `bus ${d.direction ?? '?'} n${d.node ?? '?'} seq=${d.uart_seq ?? '?'} ${messageTypeLabel(d.message_type)} ${d.result ?? ''}${d.raw_hex ? ` [${d.raw_hex}]` : ''}`;
		default:
			return env.type;
	}
}

// One line of the debug console: summarize() supplies the text, levelOf() the colour.
export interface TickEntry {
	id: number;
	at: string;
	text: string;
	level: 'info' | 'warn' | 'error' | 'event';
}

export const SERVER_ERRORS: Record<string, { text: string; level: TickEntry['level'] }> = {
	shed_slow_client: { text: 'dropped by server: this client could not keep up', level: 'error' },
	shed_lagged: { text: 'dropped by server: fell behind the event stream', level: 'error' },
	unknown_event_type: { text: 'server rejected an unknown event type', level: 'warn' }
};

// Classify an envelope for log colour-coding in the debug console.
export function levelOf(env: Envelope): TickEntry['level'] {
	if (env.type === 'diagnostic.log') {
		const l = (env.data?.level ?? '').toLowerCase();
		if (l === 'error' || l === 'fatal') return 'error';
		if (l === 'warn' || l === 'warning') return 'warn';
	}
	if (env.type === 'command.result' && env.status && env.status !== 'applied') return 'warn';
	if (env.type === 'calibration.result' && env.data?.ok === false) return 'error';
	return 'event';
}
