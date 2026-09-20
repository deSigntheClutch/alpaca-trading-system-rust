/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Set to "false" to hit the real backtest API instead of synthetic data. */
  readonly VITE_USE_FIXTURES?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
