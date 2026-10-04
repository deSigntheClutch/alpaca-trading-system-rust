import { defineConfig, globalIgnores } from 'eslint/config'
import nextVitals from 'eslint-config-next/core-web-vitals'
import nextTs from 'eslint-config-next/typescript'

export default defineConfig([
  ...nextVitals,
  ...nextTs,
  globalIgnores(['.next/**', 'out/**', 'next-env.d.ts', 'src/bindings/**']),
  {
    rules: {
      // The React Compiler rules are stricter than the React 18 code was written
      // against. App.tsx kicks off the first backtest from an effect, which this
      // rule flags. Kept as a warning until that is restructured.
      'react-hooks/set-state-in-effect': 'warn',
    },
  },
])
