import { useTranslation } from "react-i18next"
import { useDocumentTitle } from "@/lib/use-document-title"

/**
 * Teacher onboarding guide. Walks a teacher through wiring Minerva into a
 * Moodle course end to end (add the activity, link materials, first launch).
 * The screenshots live in `public/help/` and are captured against the real
 * dev stacks; keep them in sync with the LaTeX guide under `docs/`.
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

export function TeacherHelpPage() {
  const { t } = useTranslation("common")
  useDocumentTitle(t("pageTitles.teacherHelp"))

  return (
    <div className="max-w-3xl mx-auto">
      <h1 className="text-2xl font-bold tracking-tight mb-3">
        {t("teacherGuide.title")}
      </h1>
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

      <ol className="mt-8 space-y-12">
        <li>
          <h2 className="text-lg font-semibold">
            {t("teacherGuide.steps.login.title")}
          </h2>
          <p className="mt-1 text-sm leading-relaxed text-muted-foreground">
            {t("teacherGuide.steps.login.body")}
          </p>
          <Shot
            src="/help/minerva-dashboard.png"
            alt={t("teacherGuide.steps.login.alt")}
          />
        </li>

        <li>
          <h2 className="text-lg font-semibold">
            {t("teacherGuide.steps.addActivity.title")}
          </h2>
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
          <h2 className="text-lg font-semibold">
            {t("teacherGuide.steps.linkMaterials.title")}
          </h2>
          <p className="mt-1 text-sm leading-relaxed text-muted-foreground">
            {t("teacherGuide.steps.linkMaterials.body")}
          </p>
          <Shot
            src="/help/link-form.png"
            alt={t("teacherGuide.steps.linkMaterials.alt")}
          />
          <p className="mt-4 text-sm leading-relaxed text-muted-foreground">
            {t("teacherGuide.steps.linkMaterials.body2")}
          </p>
          <Shot
            src="/help/linked.png"
            alt={t("teacherGuide.steps.linkMaterials.altLinked")}
          />
          <Shot
            src="/help/sync-complete.png"
            alt={t("teacherGuide.steps.linkMaterials.altSync")}
          />
        </li>

        <li>
          <h2 className="text-lg font-semibold">
            {t("teacherGuide.steps.firstLaunch.title")}
          </h2>
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
        <h2 className="text-lg font-semibold">
          {t("teacherGuide.result.title")}
        </h2>
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
        <h2 className="text-lg font-semibold">{t("teacherGuide.tips.title")}</h2>
        <ul className="mt-2 list-disc pl-5 space-y-2 text-sm leading-relaxed text-muted-foreground">
          <li>{t("teacherGuide.tips.materials")}</li>
          <li>{t("teacherGuide.tips.resync")}</li>
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
