import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// Dev server proxies /api → the axum bin (bin/web, :8080), so the SPA fetches
// same-origin in dev and prod alike.
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': 'http://127.0.0.1:8080',
    },
  },
})
