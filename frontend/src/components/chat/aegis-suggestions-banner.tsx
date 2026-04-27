/**
 * Above-the-input banner that surfaces Aegis suggestions for the
 * student's current draft. Replaces the previous inline amber
 * sentence under the Send button; the same information now sits
 * visibly above the input where Slack/iMessage place "replying to"
 * or "attached file" chips, so the student SEES it without having
 * to glance away from the input.
 *
 * Three visual states:
 *   * `idle`    ; suggestions exist for the draft. Soft amber
 *                   tile, "Aegis has N ideas" header, "Some ideas"
 *                   button on the right, dismiss X. Never blocks
 *                   the send.
 *   * `blocked` ; the student pressed Send and got soft-blocked.
 *                   Same tile but a touch more prominent (rose
 *                   border) and the secondary text changes to
 *                   "Press Send again to send as-is".
 *   * `working` ; "Some ideas" is in flight. Button shows
 *                   "Rewriting..." + disabled. Reverts to idle on
 *                   completion.
 *
 * The dismiss X collapses the banner for THIS draft only; new
 * input regenerates a fresh verdict and the banner can return.
 */
import { useTranslation } from "react-i18next"
import { Wand2, X } from "lucide-react"
import { Button } from "@/components/ui/button"
import { AegisShieldFilled } from "@/components/icons/aegis-shield-filled"
import { cn } from "@/lib/utils"

type BannerState = "idle" | "blocked" | "working"

interface AegisSuggestionsBannerProps {
  /** Number of active suggestions; drives the count in the header. */
  suggestionCount: number
  /** True when the student pressed Send while suggestions were active. */
  blocked: boolean
  /** True while a rewrite request is in flight. */
  working: boolean
  /** Click "Some ideas"; triggers POST /aegis/rewrite + auto-send. */
  onUseIdeas: () => void
  /** Collapse the banner for the current draft. */
  onDismiss: () => void
}

export function AegisSuggestionsBanner({
  suggestionCount,
  blocked,
  working,
  onUseIdeas,
  onDismiss,
}: AegisSuggestionsBannerProps) {
  const { t } = useTranslation("student")
  const state: BannerState = working ? "working" : blocked ? "blocked" : "idle"

  // Tile colour reflects the urgency: amber for idle / working,
  // rose for the soft-block state. Same palette family the
  // suggestion cards use so the banner reads as part of the same
  // visual system.
  const containerClass = cn(
    "flex items-center gap-2 rounded-md border px-3 py-2",
    state === "blocked"
      ? "border-rose-300 bg-rose-50 dark:bg-rose-950/40 dark:border-rose-800"
      : "border-amber-300 bg-amber-50 dark:bg-amber-950/40 dark:border-amber-800",
  )

  return (
    <div className={containerClass} role="status" aria-live="polite">
      <AegisShieldFilled
        size={20}
        className="rounded shrink-0"
        aria-hidden="true"
      />
      <div className="flex-1 min-w-0">
        <div className="text-sm font-medium leading-tight">
          {t("aegis.banner.title", { count: suggestionCount })}
        </div>
        <div className="text-xs text-muted-foreground leading-tight">
          {state === "blocked"
            ? t("aegis.banner.blockedHint")
            : t("aegis.banner.idleHint")}
        </div>
      </div>
      <Button
        type="button"
        size="sm"
        variant="default"
        onClick={onUseIdeas}
        disabled={working}
        className="shrink-0 gap-1.5"
      >
        <Wand2 className="w-3.5 h-3.5" />
        {working ? t("aegis.banner.working") : t("aegis.banner.useIdeas")}
      </Button>
      <Button
        type="button"
        size="sm"
        variant="ghost"
        onClick={onDismiss}
        disabled={working}
        aria-label={t("aegis.banner.dismiss")}
        title={t("aegis.banner.dismiss")}
        className="h-7 w-7 p-0 shrink-0"
      >
        <X className="h-4 w-4" />
      </Button>
    </div>
  )
}
