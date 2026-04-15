import "@github/relative-time-element/define"

export function RelativeTime({ date }: { date: string | Date }) {
  const d = typeof date === "string" ? new Date(date) : date
  return <relative-time datetime={d.toISOString()}>{d.toLocaleString()}</relative-time>
}
