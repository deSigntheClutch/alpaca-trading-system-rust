import type { NextConfig } from 'next'
import { PHASE_DEVELOPMENT_SERVER } from 'next/constants'

/**
 * Two deployment shapes, one config. Next lets the config be a function of the
 * build phase, which is how the two shapes stay in one file without warnings.
 *
 * Dev (`next dev`, :3000): the axum bin runs on :8080. The rewrite proxies
 *   /api/* there, so the browser fetches same-origin and there is no CORS to
 *   think about. This replaces Vite's `server.proxy`.
 *
 * Prod (`next build`): `output: 'export'` writes plain HTML/JS/CSS into `out/`,
 *   and the axum bin serves that folder as static files. No Node server runs in
 *   prod, which is why the page is a client-rendered SPA and does not use Server
 *   Components for data. Rewrites cannot apply to a static export and are not
 *   needed there: the API is already same-origin. `trailingSlash` makes every
 *   route a folder with an `index.html` (`out/backtest/index.html`), which is the
 *   shape a plain static file server resolves; without it `/backtest` is a 404.
 */
export default function config(phase: string): NextConfig {
  if (phase === PHASE_DEVELOPMENT_SERVER) {
    return {
      async rewrites() {
        return [{ source: '/api/:path*', destination: 'http://127.0.0.1:8080/api/:path*' }]
      },
    }
  }
  return { output: 'export', trailingSlash: true }
}
