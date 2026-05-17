/**
 * Empty-state hero shown above the chat input when the user lands
 * on a brand-new conversation. Time-of-day greeting + first-name
 * personalisation + LLM-generated starter chips. Used by both the
 * Shibboleth chat page and the LTI/embed iframe; strings live in
 * the `student` namespace so we don't duplicate keys across
 * `auth.json`.
 */
import { useTranslation } from "react-i18next"

import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"

type TimeOfDay = "morning" | "afternoon" | "evening" | "night"

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
  suggestions,
  onSuggestionClick,
  className,
}: {
  // We render only the first whitespace-separated token because
  // "Good morning, Edwin Sundberg!" reads stilted; for `ext:` users
  // the backend pseudonymiser returns a two-word EFF-wordlist name
  // and the first token still reads naturally.
  displayName: string | null | undefined
  courseName?: string | null
  suggestions?: string[]
  onSuggestionClick?: (question: string) => void
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
      {(suggestions?.length ?? 0) > 0 && (
        <div className="mt-6 w-full max-w-2xl">
          <p className="text-xs uppercase tracking-wide text-muted-foreground mb-2">
            {t("greeting.suggestionsLabel")}
          </p>
          <div className="flex flex-wrap justify-center gap-2">
            {suggestions!.map((q, i) => (
              <Button
                key={`${i}-${q}`}
                type="button"
                variant="outline"
                size="sm"
                onClick={() => onSuggestionClick?.(q)}
                disabled={!onSuggestionClick}
                className="rounded-full max-w-full"
              >
                <span className="truncate">{q}</span>
              </Button>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}
