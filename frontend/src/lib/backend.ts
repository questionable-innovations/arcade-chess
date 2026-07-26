// Which backend this client talks to, and the admin password remembered for it.

const AUTH_PASSWORD_KEY_PREFIX = 'arcade-chess.admin-password:';

export function resolveUrl(): string {
	const override = new URLSearchParams(location.search).get('backend');
	if (override) return override;
	const h = location.hostname;
	if (h === 'localhost' || h === '127.0.0.1') return 'ws://localhost:8080/ws';
	return 'wss://chess-be.qinnovate.nz/ws';
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
