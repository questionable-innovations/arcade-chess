// ── Offline demo harness ──────────────────────────────────────────────────────
// Builds a plausible full-board state so the interface can be reviewed and
// screenshotted without a live device. Activated by the `?demo` query flag.

import { emptyDevice, NODE_COUNT, SQUARE_COUNT, type DeviceState, type Envelope } from './types';

export function demoDevice(): DeviceState {
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

	for (let n = 0; n < NODE_COUNT; n++) {
		dev.node_status[n] = {
			type: 'node.status',
			seq: 100 + n,
			at_ms: 60000,
			data: {
				node: n,
				online: true,
				calibrated: n !== 2,
				firmware: '0.1.0',
				node_uptime_ms: 4180000 + n * 1500,
				event_depth: 0,
				last_scan_ms: 11 + n,
				node_rx_good: 8120 + n * 40,
				node_rx_bad: 0,
				node_event_overflow: 0,
				supply_mv: 4980 - n * 15,
				timeouts: 0,
				reboots: 0
			}
		};
	}
	dev.device_status = {
		type: 'device.status',
		data: {
			rssi: -58,
			heap: 184320,
			uptime: 4210000,
			uptime_ms: 4210000,
			uart_good: 32480,
			uart_bad: 0,
			uart_timeouts: 0,
			ws_send_failed: 0,
			events_dropped_offline: 0,
			snapshot_repairs: 0,
			raw_stream: false,
			trace: false
		}
	};
	// Op-amp centres each square at ~512 (2.5 V mid-rail); a piece pushes the
	// reading ±~285 counts by polarity. Empty squares hover in the deadband.
	dev.raw_scan = {
		type: 'sensor.raw_scan',
		data: {
			scan_id: 7,
			complete: true,
			response_node_mask: 0b1111,
			baseline_adc: Array.from({ length: SQUARE_COUNT }, () => 512),
			noise_adc: Array.from({ length: SQUARE_COUNT }, (_, i) => 3 + ((i * 7) % 4)),
			raw_adc: Array.from({ length: SQUARE_COUNT }, (_, i) => {
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

	return dev;
}

export function demoEvents(dev: DeviceState): Envelope[] {
	return [
		{
			type: 'device.status',
			data: { rssi: -58, heap: 184320, uptime_ms: 4210000 },
			at_ms: 60000
		},
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
	];
}
