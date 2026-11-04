import { useTranslation } from "react-i18next"

/** `2 Feb 2027` / `2 feb. 2027`. */
export function useDateFormat() {
  const { i18n } = useTranslation()
  const fmt = new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium" })
  return (date: string | Date) => fmt.format(typeof date === "string" ? new Date(date) : date)
}

/** When the retention sweep deletes a case untouched since `updatedAt`. */
export function deletionDate(updatedAt: string, retentionDays: number): Date {
  return new Date(new Date(updatedAt).getTime() + retentionDays * 86_400_000)
}
