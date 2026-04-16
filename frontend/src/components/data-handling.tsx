import { Trans, useTranslation } from "react-i18next"

/**
 * Shared disclosure copy rendered both on the standalone `/data-handling`
 * page and inside the student first-use modal. Factored into a component so
 * the two stay in sync.
 */
export function DataHandlingContent() {
  const { t } = useTranslation("common")
  return (
    <div className="space-y-5 text-sm leading-relaxed">
      <section className="space-y-2">
        <h2 className="font-semibold text-base">
          {t("dataHandling.whatWeStore.heading")}
        </h2>
        <ul className="list-disc pl-5 space-y-1 text-muted-foreground">
          <li>{t("dataHandling.whatWeStore.identity")}</li>
          <li>{t("dataHandling.whatWeStore.conversations")}</li>
          <li>{t("dataHandling.whatWeStore.materials")}</li>
        </ul>
      </section>

      <section className="space-y-2">
        <h2 className="font-semibold text-base">
          {t("dataHandling.staffAccess.heading")}
        </h2>
        <p className="text-muted-foreground">
          {t("dataHandling.staffAccess.body")}
        </p>
      </section>

      <section className="space-y-2">
        <h2 className="font-semibold text-base">
          {t("dataHandling.whereMessagesGo.heading")}
        </h2>
        <ul className="list-disc pl-5 space-y-1 text-muted-foreground">
          <li>
            <Trans
              ns="common"
              i18nKey="dataHandling.whereMessagesGo.cerebras"
              components={{ strong: <strong /> }}
            />
          </li>
          <li>
            <Trans
              ns="common"
              i18nKey="dataHandling.whereMessagesGo.openai"
              components={{ strong: <strong /> }}
            />
          </li>
          <li>{t("dataHandling.whereMessagesGo.nothingElse")}</li>
        </ul>
      </section>

      <section className="space-y-2">
        <h2 className="font-semibold text-base">
          {t("dataHandling.retention.heading")}
        </h2>
        <p className="text-muted-foreground">
          {t("dataHandling.retention.body")}
        </p>
      </section>

      <section className="space-y-2">
        <h2 className="font-semibold text-base">
          {t("dataHandling.integrations.heading")}
        </h2>
        <ul className="list-disc pl-5 space-y-1 text-muted-foreground">
          <li>
            <Trans
              ns="common"
              i18nKey="dataHandling.integrations.canvas"
              components={{ strong: <strong /> }}
            />
          </li>
          <li>
            <Trans
              ns="common"
              i18nKey="dataHandling.integrations.moodle"
              components={{ strong: <strong /> }}
            />
          </li>
          <li>
            <Trans
              ns="common"
              i18nKey="dataHandling.integrations.lti"
              components={{ strong: <strong /> }}
            />
          </li>
          <li>
            <Trans
              ns="common"
              i18nKey="dataHandling.integrations.play"
              components={{ strong: <strong /> }}
            />
          </li>
        </ul>
      </section>

      <section className="space-y-2">
        <h2 className="font-semibold text-base">
          {t("dataHandling.contact.heading")}
        </h2>
        <p className="text-muted-foreground">
          {t("dataHandling.contact.questions")}{" "}
          <a
            href="mailto:lambda@dsv.su.se"
            className="underline hover:text-foreground"
          >
            lambda@dsv.su.se
          </a>
        </p>
      </section>
    </div>
  )
}
