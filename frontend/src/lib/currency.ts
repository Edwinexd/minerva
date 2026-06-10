/**
 * USD formatting for spending caps and per-model prices. Spend/limits
 * are dollars (small values, up to 4 dp for sub-cent per-Mtok prices);
 * we render with a currency style so the UI reads as money, not a bare
 * number.
 */
const usdFmt = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 2,
  maximumFractionDigits: 4,
})

export function formatUsd(value: number): string {
  return usdFmt.format(value)
}
