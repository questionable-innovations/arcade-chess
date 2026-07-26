// Device-state reducer: folds relayed event envelopes into a DeviceState.

import { NODE_COUNT, SQUARE_COUNT, type DeviceState, type Envelope, type EventData } from './types';

function rebuildSquares(dev: DeviceState, d: EventData): void {
	const sq = d.squares ?? [];
	const valid = d.valid ?? [];
	for (let i = 0; i < SQUARE_COUNT; i++) {
		const v = sq[i];
		dev.squares[i] = v === 1 ? 'positive' : v === -1 ? 'negative' : 'empty';
		dev.valid[i] = valid.length ? !!valid[i] : true;
	}
}

function rebuildNodes(dev: DeviceState, env: Envelope, d: EventData): void {
	for (const summary of d.nodes ?? []) {
		if (!Number.isInteger(summary.node) || summary.node < 0 || summary.node >= NODE_COUNT) continue;
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

// The quadrant this event addresses, or null when it names no usable node.
function nodeIndex(d: EventData): number | null {
	const n = d.node;
	return typeof n === 'number' && n >= 0 && n < NODE_COUNT ? n : null;
}

// Rebuild device state from one relayed event, honouring (boot_id, seq) continuity:
// snapshots supersede everything; a seq gap freezes sensor.changed until the next snapshot.
export function applyEvent(dev: DeviceState, env: Envelope): void {
	const d = env.data ?? {};
	const node = nodeIndex(d);
	if (env.type === 'node.status' && node != null) {
		dev.node_status[node] = env;
	} else if (env.type === 'device.status') {
		dev.device_status = env;
	} else if (env.type === 'sensor.raw_scan') {
		dev.raw_scan = env;
	} else if (env.type === 'calibration.progress' && node != null) {
		dev.calibration[node] = { active: true, percent: d.percent ?? 0 };
	} else if (env.type === 'calibration.result' && node != null) {
		dev.calibration[node] = {
			active: false,
			percent: 100,
			ok: !!d.ok,
			reason: d.reason
		};
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
			if (typeof d.square === 'number' && d.square >= 0 && d.square < SQUARE_COUNT && d.state) {
				dev.squares[d.square] = d.state;
			}
		}
	}
}
