/** Shared display formatting, so four components round numbers the same way. */

export const pct = (x: number, digits = 2) => `${(x * 100).toFixed(digits)}%`

export const money = (x: number) =>
  `${x < 0 ? '-' : ''}$${Math.abs(x).toLocaleString(undefined, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`

/** Signed, for P&L columns where the sign carries the meaning. */
export const signedMoney = (x: number) => `${x >= 0 ? '+' : '-'}$${Math.abs(x).toFixed(2)}`

export const signedPct = (x: number) => `${x >= 0 ? '+' : ''}${(x * 100).toFixed(2)}%`

export const date = (ms: number) => new Date(ms).toISOString().slice(0, 10)

/** Crypto quantities are fractional; stock quantities usually are not. */
export const qty = (q: number) => (q >= 1 ? q.toFixed(3) : q.toPrecision(3))

export const price = (p: number) => p.toLocaleString(undefined, { maximumFractionDigits: 2 })

/** Whole days between two timestamps, for "held 12 days". */
export const daysBetween = (a: number, b: number) => Math.round((b - a) / 86_400_000)
