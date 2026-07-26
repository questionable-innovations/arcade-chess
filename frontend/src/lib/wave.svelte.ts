// Board-map sweep. One pass per axis, because a single sweep only proves one of
// them: the file pass pins each square's column, the rank pass pins its row.
// A square that lights out of step is mismapped, not miscalibrated.

const BOARD_SIZE = 8;
// One frame per file, then one per rank; each frame lights a whole line.
const FILE_FRAMES = BOARD_SIZE;
const FRAMES = BOARD_SIZE * 2;
const STEP_MS = 140;
// Longer than a step so a late frame leaves no gap, and short enough that the
// sweep self-clears on the nodes if the socket dies mid-run.
const HOLD_MS = 260;
const FILE_COLOUR = '00a0ff';
const RANK_COLOUR = 'ffa000';

interface WaveHost {
	/** Returns false once the link is down, which strands the run. */
	send(obj: unknown): boolean;
	notify(text: string, level: 'info' | 'warn' | 'error'): void;
}

// Drives the sweep entirely client-side, so unlike the stream/trace toggles there
// is no device report to reconcile `active` against.
export class WaveRunner {
	active = $state(false);
	#timer: ReturnType<typeof setTimeout> | null = null;
	#host: WaveHost;

	constructor(host: WaveHost) {
		this.#host = host;
	}

	// Sweep a lit file left to right, then a lit rank bottom to top, to prove the
	// board map end to end: global square -> node -> local square -> LED chain.
	// Calling it again while running stops it.
	start(deviceId: string): void {
		if (this.active) return this.stop(deviceId);
		this.active = true;
		let frame = 0;
		const step = () => {
			this.#timer = null;
			if (!this.active) return;
			if (frame >= FRAMES) return this.stop(deviceId);
			// Board indices are row * BOARD_SIZE + col, row 0 = rank 1, col 0 = file a.
			const files = frame < FILE_FRAMES;
			const line = files ? frame : frame - FILE_FRAMES;
			const squares: number[] = [];
			for (let n = 0; n < BOARD_SIZE; n++) {
				squares.push(files ? n * BOARD_SIZE + line : line * BOARD_SIZE + n);
			}
			const sent = this.#host.send({
				type: 'command',
				device_id: deviceId,
				name: 'lighting.set',
				args: {
					squares,
					effect: 'solid',
					colour: files ? FILE_COLOUR : RANK_COLOUR,
					duration_ms: HOLD_MS
				}
			});
			// A dead socket would otherwise leave the timer spinning against nothing.
			if (!sent) {
				this.active = false;
				this.#host.notify('wave stopped — link is down', 'warn');
				return;
			}
			frame++;
			this.#timer = setTimeout(step, STEP_MS);
		};
		step();
	}

	// Safe to call unconditionally; the explicit clear covers a stop mid-sweep,
	// where the last few lines still hold their override.
	stop(deviceId: string): void {
		const wasActive = this.cancel();
		if (!wasActive) return;
		this.#host.send({ type: 'command', device_id: deviceId, name: 'lighting.clear', args: {} });
	}

	// Abandon the run without touching the bus, for when the link is already gone.
	// Nodes expire their own override within HOLD_MS, so no clear is needed.
	// Returns whether a sweep was actually running.
	cancel(): boolean {
		if (this.#timer) {
			clearTimeout(this.#timer);
			this.#timer = null;
		}
		const wasActive = this.active;
		this.active = false;
		return wasActive;
	}
}
