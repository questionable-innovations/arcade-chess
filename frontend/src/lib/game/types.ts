// The `game.state` snapshot the server broadcasts, plus the pure helpers the
// game UI needs. See docs/client-api.md §Puzzle mode.
//
// The frontend computes no chess rules. `legal_moves` arrives with every state,
// so click-to-move is "filter the server's list" and there is exactly one place
// that knows what a legal move is.

export type Phase =
	'idle' | 'setup' | 'countdown' | 'playing' | 'awaiting_choice' | 'scoring' | 'finished';

export type DetectMode = 'auto' | 'suggest' | 'off';
export type EvalSourceName = 'stockfish' | 'material' | 'admin';

export interface GameMove {
	uci: string;
	san: string;
	by: 'sensor' | 'manual' | 'chosen' | 'autopilot';
	confidence: 'certain' | 'likely';
}

export interface Candidate {
	uci: string;
	san: string;
	confidence: 'certain' | 'likely';
}

export interface GameState {
	type?: string;
	game_seq: number;
	phase: Phase;
	device_id: string | null;
	position: { id: string; start_fen: string; verified_cp?: number; drop_cp?: number } | null;
	start_fen: string;
	fen: string;
	turn: 'white' | 'black';
	ply: number;
	max_ply: number;
	moves: GameMove[];
	legal_moves: string[];
	setup?: {
		placed: number;
		needed: number;
		missing: number[];
		extra: number[];
		unknown: number[];
		auto_start_in_ms: number | null;
	};
	detect: {
		mode: DetectMode;
		sensors_live: boolean;
		board_synced: boolean;
		mismatch: number[];
		observed: string;
		masked: number[];
		rotation: number;
		nudge: { expected: number; actual: number } | null;
	};
	choice?: {
		kind: 'capture' | 'promotion' | 'no_match' | 'suggest';
		prompt: string;
		options: Candidate[];
	};
	eval: {
		cp: number;
		mate: number | null;
		win_prob: number;
		status: 'ok' | 'pending';
		source: EvalSourceName;
		depth: number;
		start_cp: number;
	};
	result?: {
		winner: 'white' | 'black' | 'draw';
		final_cp: number;
		start_cp: number;
		swing: number;
		reason: string;
	};
	tunables: {
		settle_ms: number;
		autostart_stable_ms: number;
		unknown_tolerance: number;
		tier3_max_distance: number;
		tier3_margin: number;
		draw_band_cp: number;
		countdown_ms: number;
	};
	lighting: {
		squares: string;
		bars_supported: boolean;
		bar_map: { node: number; strip: string; reversed: boolean }[][];
		colours_neutralised: boolean;
	};
	autopilot: { interval_ms: number } | null;
	deck: { source: string; count: number; skipped: number };
	degraded: string[];
}

export const IDLE_GAME: GameState = {
	game_seq: 0,
	phase: 'idle',
	device_id: null,
	position: null,
	start_fen: '8/8/8/8/8/8/8/8 w - - 0 1',
	fen: '8/8/8/8/8/8/8/8 w - - 0 1',
	turn: 'white',
	ply: 0,
	max_ply: 10,
	moves: [],
	legal_moves: [],
	detect: {
		mode: 'auto',
		sensors_live: false,
		board_synced: true,
		mismatch: [],
		observed: 'x'.repeat(64),
		masked: [],
		rotation: 0,
		nudge: null
	},
	eval: {
		cp: 0,
		mate: null,
		win_prob: 0.5,
		status: 'pending',
		source: 'material',
		depth: 0,
		start_cp: 0
	},
	tunables: {
		settle_ms: 700,
		autostart_stable_ms: 1500,
		unknown_tolerance: 0,
		tier3_max_distance: 1,
		tier3_margin: 1,
		draw_band_cp: 40,
		countdown_ms: 3000
	},
	lighting: {
		squares: 'override',
		bars_supported: true,
		bar_map: [],
		colours_neutralised: false
	},
	autopilot: null,
	deck: { source: 'embedded', count: 0, skipped: 0 },
	degraded: []
};

// ── Board geometry ─────────────────────────────────────────────────────────
// One convention, everywhere: square index == FEN index with a1 = 0, which is
// also the device square index and `shakmaty::Square as u8`. No conversions.

export const FILES = 'abcdefgh';

export function squareName(index: number): string {
	return `${FILES[index % 8]}${Math.floor(index / 8) + 1}`;
}

export function squareIndex(name: string): number {
	return (Number(name[1]) - 1) * 8 + FILES.indexOf(name[0]);
}

// ── Pieces ─────────────────────────────────────────────────────────────────
// Pieces render as SVGs from `static/pieces/`. Unicode glyphs were the obvious
// zero-asset choice and they do not survive contact with a dark board seen from
// across a room: the solid black forms vanish into the square and the outline
// white forms lose their silhouette. Twelve small SVGs cost 52 KB.

const PIECE_LETTERS = 'KQRBNPkqrbnp';

export interface Piece {
	white: boolean;
	letter: string;
}

/** Expands a FEN's board field into 64 slots, a1 first. */
export function boardFromFen(fen: string): (Piece | null)[] {
	const squares: (Piece | null)[] = Array.from({ length: 64 }, () => null);
	const field = fen.split(' ')[0] ?? '';
	let rank = 7;
	let file = 0;
	for (const char of field) {
		if (char === '/') {
			rank -= 1;
			file = 0;
		} else if (char >= '1' && char <= '8') {
			file += Number(char);
		} else {
			if (PIECE_LETTERS.includes(char) && rank >= 0 && file < 8) {
				squares[rank * 8 + file] = {
					white: char === char.toUpperCase(),
					letter: char
				};
			}
			file += 1;
		}
	}
	return squares;
}

/** Where a UCI move starts and ends. Promotion suffixes are ignored. */
export function uciSquares(uci: string): [number, number] | null {
	if (uci.length < 4) return null;
	const from = squareIndex(uci.slice(0, 2));
	const to = squareIndex(uci.slice(2, 4));
	if (from < 0 || to < 0 || Number.isNaN(from) || Number.isNaN(to)) return null;
	return [from, to];
}

/** Every legal destination for a piece, straight off the server's move list. */
export function targetsFrom(legal: string[], from: number): number[] {
	const out = new Set<number>();
	for (const uci of legal) {
		const pair = uciSquares(uci);
		if (pair && pair[0] === from) out.add(pair[1]);
	}
	return [...out];
}

/**
 * The UCI string for a click, preferring the queen promotion. Occupancy cannot
 * see an underpromotion either, so the two input paths agree.
 */
export function pickMove(legal: string[], from: number, to: number): string | null {
	const matches = legal.filter((uci) => {
		const pair = uciSquares(uci);
		return pair && pair[0] === from && pair[1] === to;
	});
	if (!matches.length) return null;
	return matches.find((uci) => uci.endsWith('q')) ?? matches[0];
}

// ── Presentation ───────────────────────────────────────────────────────────

/** `+1.34`, `M3`, or `—` while the first search is still out. */
export function formatEval(state: GameState): string {
	if (state.eval.status !== 'ok') return '—';
	if (state.eval.mate != null) return `M${Math.abs(state.eval.mate)}`;
	const pawns = state.eval.cp / 100;
	return `${pawns >= 0 ? '+' : ''}${pawns.toFixed(2)}`;
}

/**
 * Plain English for a `degraded` chip. Anything unrecognised is shown raw
 * rather than swallowed — an unnamed failure is the thing this list exists to
 * prevent.
 */
export function degradedLabel(code: string): string {
	const fixed: Record<string, string> = {
		no_device: 'no board bound',
		sensors_stale: 'board silent',
		engine_unavailable: 'engine down — material eval',
		bars_unsupported: 'edge bars unsupported',
		positions_fallback: 'fallback puzzle deck',
		restored_after_restart: 'restored after a restart',
		detect_suggest: 'detection: confirm each move',
		detect_off: 'detection off — click to move'
	};
	if (fixed[code]) return fixed[code];
	const offline = /^node(\d)_offline$/.exec(code);
	if (offline) return `quadrant ${offline[1]} offline`;
	const suspect = /^sensor_(\d+)_suspect$/.exec(code);
	if (suspect) return `${squareName(Number(suspect[1]))} sensor suspect`;
	const masked = /^sensor_(\d+)_masked$/.exec(code);
	if (masked) return `${squareName(Number(masked[1]))} masked`;
	return code;
}

/** Sensor readings as the server sees them: `.` `+` `-` `?` `x`. */
export function observedAt(state: GameState, square: number): string {
	return state.detect.observed[square] ?? 'x';
}
