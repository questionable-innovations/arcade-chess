// Which backend this client talks to, and the admin password remembered for it.

const AUTH_PASSWORD_KEY_PREFIX = 'arcade-chess.admin-password:';

export function resolveUrl(): string {
	const override = new URLSearchParams(location.search).get('backend');
	if (override) return override;
	const h = location.hostname;
	if (h === 'localhost' || h === '127.0.0.1') return 'ws://localhost:8080/ws';
	return 'wss://chess-be.qinnovate.nz/ws';
}

/**
 * Projector mode: `?projector` scales the page up and drops the operator
 * chrome, so one build serves both the phone in the operator's hand and the
 * screen at the back of the room. "Readable across a room" is a stated
 * requirement, and a deliberately muted palette is exactly the choice that
 * dies under a projector — so this has to be switchable in the room rather
 * than at build time.
 */
export function isProjector(): boolean {
	// The page is prerendered to static assets, so this runs once in Node with
	// no `location` at all before it ever runs in a browser.
	if (typeof location === 'undefined') return false;
	return new URLSearchParams(location.search).has('projector');
}

function authPasswordKey(): string {
	return `${AUTH_PASSWORD_KEY_PREFIX}${resolveUrl()}`;
}

export function rememberedPassword(): string | null {
	try {
		return localStorage.getItem(authPasswordKey());
	} catch {
		return null;
	}
}

export function rememberPassword(password: string): void {
	try {
		localStorage.setItem(authPasswordKey(), password);
	} catch {
		// Authentication still works when storage is unavailable.
	}
}

export function forgetPassword(password: string): void {
	try {
		const key = authPasswordKey();
		if (localStorage.getItem(key) === password) localStorage.removeItem(key);
	} catch {
		// Nothing to clear when storage is unavailable.
	}
}
