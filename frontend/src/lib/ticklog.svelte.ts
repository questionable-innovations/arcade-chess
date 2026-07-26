// The debug console's two ring buffers, both newest-first.

import { levelOf, summarize, type Envelope, type TickEntry } from './types';

const LOG_MAX = 250;
const BUS_LOG_MAX = 400;

export interface BusFrame {
	id: number;
	env: Envelope;
}

export class TickLog {
	entries = $state<TickEntry[]>([]);
	// Raw UART frame trace (diagnostic.bus), kept out of the main ticker so a
	// 40 Hz trace doesn't drown semantic events. Client-side id: uart/device seqs
	// reset on reboot, so they can't key the list.
	busFrames = $state<BusFrame[]>([]);
	#tickId = 0;
	#busId = 0;

	push(env: Envelope): void {
		this.entries = [this.#tick(env), ...this.entries].slice(0, LOG_MAX);
	}

	pushInfo(text: string, level: TickEntry['level'] = 'info'): void {
		this.entries = [this.#info(text, level), ...this.entries].slice(0, LOG_MAX);
	}

	pushBus(env: Envelope): void {
		this.busFrames = [this.#frame(env), ...this.busFrames].slice(0, BUS_LOG_MAX);
	}

	clearBus(): void {
		this.busFrames = [];
	}

	// Replace the ticker outright, for the offline demo harness.
	reset(envs: Envelope[]): void {
		this.entries = envs.map((env) => this.#tick(env));
	}

	// Seed from an `init` replay. Both inputs are oldest-first.
	//
	// A reconnect is exactly when the operator most needs the preceding log, so
	// the old entries stay below a separator instead of being replaced. The
	// stream renders newest-first, so everything under the mark is pre-drop.
	seed(semantic: Envelope[], bus: Envelope[], mark: string): void {
		const ticks = semantic.slice(-LOG_MAX).map((env) => this.#tick(env));
		const carried = this.entries.length ? [this.#info(mark), ...this.entries] : this.entries;
		this.entries = [...ticks.reverse(), ...carried].slice(0, LOG_MAX);
		const replay = bus.map((env) => this.#frame(env));
		this.busFrames = [...replay.reverse(), ...this.busFrames].slice(0, BUS_LOG_MAX);
	}

	#tick(env: Envelope): TickEntry {
		return {
			id: this.#tickId++,
			at: env.at_ms != null ? String(env.at_ms) : '·',
			text: summarize(env),
			level: levelOf(env)
		};
	}

	#info(text: string, level: TickEntry['level'] = 'info'): TickEntry {
		return { id: this.#tickId++, at: '·', text, level };
	}

	#frame(env: Envelope): BusFrame {
		return { id: this.#busId++, env };
	}
}
