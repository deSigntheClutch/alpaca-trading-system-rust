/**
 * `/backtest` keeps the FE-1/FE-2 backtest page (run controls, price chart with
 * markers, trade table, decision drill-down) reachable while the morning report
 * takes over `/`.
 */
import { App } from '@/App'

export default function BacktestPage() {
  return <App />
}
