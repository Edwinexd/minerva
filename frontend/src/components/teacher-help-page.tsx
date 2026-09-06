import { useTranslation } from "react-i18next"
import { useDocumentTitle } from "@/lib/use-document-title"

/**
 * Teacher onboarding guide. The per-course setup is down to three Moodle
 * steps: the Daisy import, the plugin's external-id auto-link and the
 * twice-hourly material sync do the rest, so the guide leads with what
 * already happens on its own and only then asks the teacher to do
 * something. The screenshots live in `public/help/` and are captured
 * against the real dev stacks; keep them in sync with the LaTeX guide
 * under `docs/`.
 */
function Shot({ src, alt }: { src: string; alt: string }) {
  return (
    <img
      src={src}
      alt={alt}
      loading="lazy"
      className="mt-3 w-full rounded-md border shadow-sm"
    />
  )
}

/**
 * `embedded` renders the guide as a section of the teacher portal, which
 * already owns the page h1 and the document title: the guide title drops
 * to an h2, every step heading follows it down one level, and the column
 * stops centring itself so it lines up with the portal's other tabs.
 */
export function TeacherHelpPage({ embedded = false }: { embedded?: boolean }) {
  const { t } = useTranslation("common")
  const Title = embedded ? "h2" : "h1"
  const Section = embedded ? "h3" : "h2"
  useDocumentTitle(embedded ? undefined : t("pageTitles.teacherHelp"))

  return (
    <div className={embedded ? "max-w-3xl" : "max-w-3xl mx-auto"}>
      <Title className="text-2xl font-bold tracking-tight mb-3">
        {t("teacherGuide.title")}
      </Title>
      <p className="text-sm leading-relaxed text-muted-foreground">
        {t("teacherGuide.intro")}
      </p>
      <p className="mt-3">
        <span className="inline-block rounded-full border px-3 py-1 text-xs text-muted-foreground">
          {t("teacherGuide.timeBadge")}
        </span>
      </p>
      <div className="mt-4 rounded-md border bg-muted/40 px-4 py-3 text-sm text-muted-foreground">
        {t("teacherGuide.adminNote")}
      </div>

      <section className="mt-8">
        <Section className="text-lg font-semibold">
          {t("teacherGuide.automatic.title")}
        </Section>
        <ul className="mt-2 list-disc pl-5 space-y-2 text-sm leading-relaxed text-muted-foreground">
          <li>{t("teacherGuide.automatic.items.courses")}</li>
          <li>{t("teacherGuide.automatic.items.link")}</li>
          <li>{t("teacherGuide.automatic.items.materials")}</li>
          <li>{t("teacherGuide.automatic.items.play")}</li>
        </ul>
        <Shot
          src="/help/minerva-dashboard.png"
          alt={t("teacherGuide.automatic.alt")}
        />
      </section>

      <ol className="mt-12 space-y-12">
        <li>
          <Section className="text-lg font-semibold">
            {t("teacherGuide.steps.enableTool.title")}
          </Section>
          <p className="mt-1 text-sm leading-relaxed text-muted-foreground">
            {t("teacherGuide.steps.enableTool.body")}
          </p>
          <Shot
            src="/help/enable-tool.png"
            alt={t("teacherGuide.steps.enableTool.alt")}
          />
          <p className="mt-4 text-sm leading-relaxed text-muted-foreground">
            {t("teacherGuide.steps.enableTool.body2")}
          </p>
        </li>

        <li>
          <Section className="text-lg font-semibold">
            {t("teacherGuide.steps.addActivity.title")}
          </Section>
          <p className="mt-1 text-sm leading-relaxed text-muted-foreground">
            {t("teacherGuide.steps.addActivity.body")}
          </p>
          <Shot
            src="/help/activity-chooser.png"
            alt={t("teacherGuide.steps.addActivity.alt")}
          />
          <p className="mt-4 text-sm leading-relaxed text-muted-foreground">
            {t("teacherGuide.steps.addActivity.body2")}
          </p>
          <Shot
            src="/help/lti-config.png"
            alt={t("teacherGuide.steps.addActivity.altConfig")}
          />
          <Shot
            src="/help/course-activity.png"
            alt={t("teacherGuide.steps.addActivity.altCourse")}
          />
        </li>

        <li>
          <Section className="text-lg font-semibold">
            {t("teacherGuide.steps.firstLaunch.title")}
          </Section>
          <p className="mt-1 text-sm leading-relaxed text-muted-foreground">
            {t("teacherGuide.steps.firstLaunch.body")}
          </p>
          <Shot
            src="/help/first-launch-bind.png"
            alt={t("teacherGuide.steps.firstLaunch.alt")}
          />
        </li>
      </ol>

      <section className="mt-12">
        <Section className="text-lg font-semibold">
          {t("teacherGuide.result.title")}
        </Section>
        <p className="mt-1 text-sm leading-relaxed text-muted-foreground">
          {t("teacherGuide.result.body")}
        </p>
        <Shot
          src="/help/student-chat.png"
          alt={t("teacherGuide.result.altChat")}
        />
        <Shot
          src="/help/launch-in-moodle.png"
          alt={t("teacherGuide.result.altInMoodle")}
        />
      </section>

      <section className="mt-12">
        <Section className="text-lg font-semibold">
          {t("teacherGuide.ownCourse.title")}
        </Section>
        <p className="mt-1 text-sm leading-relaxed text-muted-foreground">
          {t("teacherGuide.ownCourse.body")}
        </p>
        <p className="mt-3 text-sm leading-relaxed text-muted-foreground">
          {t("teacherGuide.ownCourse.body2")}
        </p>
      </section>

      <section className="mt-12">
        <Section className="text-lg font-semibold">
          {t("teacherGuide.manualLink.title")}
        </Section>
        <p className="mt-1 text-sm leading-relaxed text-muted-foreground">
          {t("teacherGuide.manualLink.body")}
        </p>
        <Shot
          src="/help/link-form.png"
          alt={t("teacherGuide.manualLink.alt")}
        />
        <p className="mt-4 text-sm leading-relaxed text-muted-foreground">
          {t("teacherGuide.manualLink.body2")}
        </p>
        <Shot src="/help/linked.png" alt={t("teacherGuide.manualLink.altLinked")} />
        <Shot
          src="/help/sync-complete.png"
          alt={t("teacherGuide.manualLink.altSync")}
        />
      </section>

      <section className="mt-12">
        <Section className="text-lg font-semibold">{t("teacherGuide.tips.title")}</Section>
        <ul className="mt-2 list-disc pl-5 space-y-2 text-sm leading-relaxed text-muted-foreground">
          <li>{t("teacherGuide.tips.materials")}</li>
          <li>{t("teacherGuide.tips.unlink")}</li>
          <li>{t("teacherGuide.tips.forums")}</li>
          <li>
            {t("teacherGuide.tips.supportLead")}{" "}
            <a
              href="mailto:lambda@dsv.su.se"
              className="underline hover:text-foreground"
            >
              lambda@dsv.su.se
            </a>
            .
          </li>
        </ul>
      </section>
    </div>
  )
}
