/**
 * Empty-state hero shown above the chat input when the user lands
 * on a brand-new conversation (no `conversationId` yet, no messages
 * pending or streaming).
 *
 * Time-of-day greeting + optional first-name personalisation,
 * mirrored in both the Shibboleth chat page and the LTI/embed
 * iframe so the two entry paths look the same. Reads its strings
 * from the `student` i18n namespace, which both routes already use
 * (the embed view borrows aegis keys from there too) so we don't
 * need to duplicate copy across `auth.json`.
 */
import { useTranslation } from "react-i18next"

import { cn } from "@/lib/utils"

type TimeOfDay = "morning" | "afternoon" | "evening" | "night"

/**
 * Bucket the current hour into one of four greeting slots. Buckets:
 *   05:00-11:59 morning, 12:00-16:59 afternoon,
 *   17:00-21:59 evening, 22:00-04:59 night (casual fallback).
 */
function getTimeOfDay(now: Date = new Date()): TimeOfDay {
  const hour = now.getHours()
  if (hour >= 5 && hour < 12) return "morning"
  if (hour >= 12 && hour < 17) return "afternoon"
  if (hour >= 17 && hour < 22) return "evening"
  return "night"
}

export function EmptyChatGreeting({
  displayName,
  courseName,
  className,
}: {
  /**
   * Full display name from `/auth/me` (or the embed `/me` shape).
   * We render only the first whitespace-separated token because
   * "Good morning, Edwin Sundberg!" reads stilted; "Good morning,
   * Edwin!" matches how a person would actually greet someone.
   * For `ext:` users the backend's pseudonymiser already returns a
   * two-word EFF-wordlist name (e.g. "Wombling Wombat"); taking
   * the first token still reads naturally there.
   */
  displayName: string | null | undefined
  /** Optional course name surfaced in the subtitle for context. */
  courseName?: string | null
  className?: string
}) {
  const { t } = useTranslation("student")
  const tod = getTimeOfDay()
  const greeting = t(`greeting.${tod}`)
  const firstName = displayName?.trim().split(/\s+/)[0] || null
  const heading = firstName
    ? t("greeting.withName", { greeting, name: firstName })
    : t("greeting.standalone", { greeting })
  const subtitle = courseName
    ? t("greeting.subtitleCourse", { course: courseName })
    : t("greeting.subtitle")

  return (
    <div
      className={cn(
        "flex flex-col items-center justify-center text-center px-6 py-12 select-none",
        className,
      )}
    >
      <h2 className="text-2xl font-semibold tracking-tight sm:text-3xl">
        {heading}
      </h2>
      <p className="mt-2 text-sm text-muted-foreground max-w-md">
        {subtitle}
      </p>
    </div>
  )
}
