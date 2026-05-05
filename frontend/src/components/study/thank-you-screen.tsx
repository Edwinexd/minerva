import { useTranslation } from "react-i18next"
import ReactMarkdown from "react-markdown"
import { Card } from "@/components/ui/card"

/**
 * Final screen rendered when the participant's study stage is
 * `done`. Shows researcher-supplied closing copy (markdown) plus a
 * standing notice that further course interaction is locked. The
 * backend enforces the lockout (`StudyLockedOut` 423 from
 * chat::send_message); this screen just informs the participant.
 */
export function ThankYouScreen({
  thankYouMarkdown,
}: {
  thankYouMarkdown: string
}) {
  const { t } = useTranslation("study")
  const trimmed = thankYouMarkdown.trim()
  return (
    <Card className="mx-auto my-8 max-w-3xl space-y-6 p-6">
      <h1 className="text-2xl font-semibold">{t("thankYou.title")}</h1>

      <div className="prose prose-sm max-w-none dark:prose-invert">
        {trimmed === "" ? (
          <p>{t("thankYou.fallbackBody")}</p>
        ) : (
          <ReactMarkdown>{thankYouMarkdown}</ReactMarkdown>
        )}
      </div>

      <p className="text-sm text-muted-foreground border-t pt-4">
        {t("thankYou.lockoutNotice")}
      </p>
    </Card>
  )
}
