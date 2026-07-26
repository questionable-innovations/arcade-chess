// Readout formatting shared by the debug rail cards.

export function kb(n: number | undefined): string {
	return n == null ? '—' : `${(n / 1024).toFixed(0)} KB`;
}

// Both the ESP and the nodes report uptime as raw millis().
export function dur(ms: number | undefined): string {
	if (ms == null) return '—';
	const s = Math.floor(ms / 1000);
	const h = Math.floor(s / 3600);
	const m = Math.floor((s % 3600) / 60);
	if (h) return `${h}h ${m}m`;
	return m ? `${m}m` : `${s}s`;
}

export function volts(mv: number | undefined): string {
	return mv == null ? '—' : `${(mv / 1000).toFixed(2)}V`;
}
