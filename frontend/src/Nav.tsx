'use client'

/**
 * The one navigation bar, shared by every route through the root layout. Tabs for
 * the pages that exist; the active one is read from the URL so the bar needs no
 * state. Paths carry the trailing slash the static export uses.
 */
import Link from 'next/link'
import { usePathname } from 'next/navigation'

const TABS = [
  { href: '/', label: 'Morning report', hint: '每天的候选和命中率' },
  { href: '/backtest/', label: 'Backtest', hint: '一只标的的历史回放' },
]

export function Nav() {
  const path = usePathname() ?? '/'
  return (
    <nav className="nav" aria-label="Pages">
      <span className="nav-brand">alpaca-trading-system</span>
      {TABS.map((t) => {
        const on = t.href === '/' ? path === '/' : path.startsWith(t.href.replace(/\/$/, ''))
        return (
          <Link key={t.href} href={t.href} className={on ? 'on' : ''} aria-current={on ? 'page' : undefined} title={t.hint}>
            {t.label}
          </Link>
        )
      })}
    </nav>
  )
}
