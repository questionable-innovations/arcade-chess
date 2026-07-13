# arcade-chess frontend

Bring-up observability UI for the sensor-driven illuminated chessboard.

Uses **pnpm** (see `.nvmrc` for the Node version — run `nvm use`).

- `pnpm install` — install dependencies.
- `pnpm dev` — local dev server (append `?backend=ws://host:port/ws` to point at another server).
- `pnpm build` — static build into `build/` (prerendered single page).
- `pnpm deploy` — build, then `wrangler deploy` to Cloudflare Workers (chess.qinnovate.nz).

Backend WebSocket: `wss://chess-be.qinnovate.nz/ws` (localhost falls back to `ws://localhost:8080/ws`).
