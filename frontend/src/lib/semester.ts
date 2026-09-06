/**
 * `VT2026` / `HT2025` semester-label helpers, shared by the My Courses
 * page and the Daisy import staging table. The label format is the one
 * `scripts/sync_daisy_courses.py` writes.
 */

/**
 * Sort key for a semester label. The year comes from the `YYYY` half;
 * HT (autumn) chronologically follows VT (spring) of the same year,
 * hence the 0.5 offset. Empty and malformed labels (shouldn't happen
 * post-server-validation, but be defensive) return -Infinity so a
 * descending sort puts them last.
 */
export function semesterSortKey(label: string): number {
  if (!label) return -Infinity
  const m = label.match(/^(VT|HT)(\d{4})$/)
  if (!m) return -Infinity
  const year = parseInt(m[2], 10)
  const seasonOffset = m[1] === "HT" ? 0.5 : 0
  return year + seasonOffset
}

export interface SemesterGroup<T> {
  /** The shared `semester_label`, or "" for the unlabelled bucket. */
  label: string
  items: T[]
}

/**
 * Bucket rows by `semester_label` and return the groups newest-first
 * (VT2027 > HT2026 > VT2026 > ...). Rows lacking a label fall into a
 * sentinel "" group that always sorts last, so an "Ad-hoc" heading
 * never hijacks the visual hierarchy.
 */
export function groupBySemester<T extends { semester_label?: string | null }>(
  rows: T[],
): SemesterGroup<T>[] {
  const buckets = new Map<string, T[]>()
  for (const row of rows) {
    const key = row.semester_label ?? ""
    if (!buckets.has(key)) buckets.set(key, [])
    buckets.get(key)!.push(row)
  }
  const entries = Array.from(buckets.entries()).map(([label, items]) => ({
    label,
    items,
  }))
  entries.sort((a, b) => {
    if (a.label === "" && b.label !== "") return 1
    if (b.label === "" && a.label !== "") return -1
    return semesterSortKey(b.label) - semesterSortKey(a.label)
  })
  return entries
}
