import type { OwnerUsage } from "./types"

/**
 * The busiest day in the reported window. A limit sized to the average
 * still cuts the course out on the day a class actually uses the tool,
 * so this is the figure both the teacher's request draft and the admin
 * setting the limit want to see.
 *
 * Lives here rather than beside the components that render it because
 * both of those files export components only (react-refresh).
 */
export function peakSpendDay(usage: OwnerUsage): number {
  return usage.daily.reduce(
    (max, d) => Math.max(max, d.chat_spend_usd + d.pipeline_spend_usd),
    0,
  )
}
