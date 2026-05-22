import { Trans, useTranslation } from "react-i18next"
import { useDocumentTitle } from "@/lib/use-document-title"

/**
 * Accessibility statement (tillgänglighetsredogörelse) required by the Swedish
 * DOS-lagen (Lag om tillgänglighet till digital offentlig service) for all
 * public-sector digital services. Structure follows DIGG's model statement:
 * compliance level, non-accessible content, how we tested, feedback channel,
 * and the enforcement (uppföljnings) procedure with a link to DIGG.
 *
 * Keep the "last assessed" date and the shortcomings list current whenever the
 * UI changes materially or a new audit is run (see docs/accessibility-audit.md).
 */
const FEEDBACK_EMAIL = "lambda@dsv.su.se"
const DIGG_REPORT_URL = "https://www.digg.se/tdosanmalan"

const SHORTCOMING_KEYS = ["contrast", "knowledgeGraph", "thirdParty"] as const

export function AccessibilityPage() {
  const { t } = useTranslation("common")
  useDocumentTitle(t("pageTitles.accessibility"))

  return (
    <div className="max-w-3xl mx-auto">
      <h1 className="text-2xl font-bold tracking-tight mb-6">
        {t("accessibility.title")}
      </h1>

      <div className="space-y-6 text-sm leading-relaxed">
        <p className="text-muted-foreground">{t("accessibility.intro")}</p>

        <section className="space-y-2">
          <h2 className="font-semibold text-base">
            {t("accessibility.complianceHeading")}
          </h2>
          <p className="text-muted-foreground">
            {t("accessibility.complianceStatus")}
          </p>
        </section>

        <section className="space-y-2">
          <h2 className="font-semibold text-base">
            {t("accessibility.shortcomingsHeading")}
          </h2>
          <p className="text-muted-foreground">
            {t("accessibility.shortcomingsIntro")}
          </p>
          <ul className="list-disc pl-5 space-y-1 text-muted-foreground">
            {SHORTCOMING_KEYS.map((key) => (
              <li key={key}>{t(`accessibility.shortcomings.${key}`)}</li>
            ))}
          </ul>
        </section>

        <section className="space-y-2">
          <h2 className="font-semibold text-base">
            {t("accessibility.testingHeading")}
          </h2>
          <p className="text-muted-foreground">
            {t("accessibility.testingBody")}
          </p>
        </section>

        <section className="space-y-2">
          <h2 className="font-semibold text-base">
            {t("accessibility.feedbackHeading")}
          </h2>
          <p className="text-muted-foreground">
            <Trans
              ns="common"
              i18nKey="accessibility.feedbackBody"
              components={{
                email: (
                  <a
                    href={`mailto:${FEEDBACK_EMAIL}`}
                    className="underline hover:text-foreground"
                  >
                    {FEEDBACK_EMAIL}
                  </a>
                ),
              }}
              values={{ email: FEEDBACK_EMAIL }}
            />
          </p>
        </section>

        <section className="space-y-2">
          <h2 className="font-semibold text-base">
            {t("accessibility.enforcementHeading")}
          </h2>
          <p className="text-muted-foreground">
            <Trans
              ns="common"
              i18nKey="accessibility.enforcementBody"
              components={{
                digg: (
                  <a
                    href={DIGG_REPORT_URL}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="underline hover:text-foreground"
                  >
                    DIGG
                  </a>
                ),
              }}
            />
          </p>
        </section>
      </div>
    </div>
  )
}
