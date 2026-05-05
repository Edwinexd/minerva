import { useQuery } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { courseQuery } from "@/lib/queries"
import { Card, CardContent } from "@/components/ui/card"
import { Skeleton } from "@/components/ui/skeleton"
import { ConfigPanel, ParticipantsPanel } from "@/components/admin/study-page"

/**
 * Per-course study management for teachers / course owners. Reuses
 * the same `<ConfigPanel>` and `<ParticipantsPanel>` the platform
 * admin tab uses; passes `canSeed={false}` to hide the DM2731
 * preset loader (admin-only). Tab is only registered when the
 * course's `study_mode` feature flag is on, so the page is
 * unreachable for non-study courses (defensive: also short-circuits
 * here if the flag flips off mid-session).
 */
export function StudyPage({
  useParams,
}: {
  useParams: () => { courseId: string }
}) {
  const { courseId } = useParams()
  const { t } = useTranslation("teacher")
  const { data: course, isLoading } = useQuery(courseQuery(courseId))

  if (isLoading) return <Skeleton className="h-64 w-full" />

  if (!course?.feature_flags?.study_mode) {
    // Flag was flipped off (or never on) for this course: nothing
    // sensible to show here. Tell the teacher who can flip it back
    // on rather than rendering a broken empty editor.
    return (
      <Card>
        <CardContent className="pt-6 space-y-2">
          <p className="text-sm">{t("study.flagOffTitle")}</p>
          <p className="text-xs text-muted-foreground">
            {t("study.flagOffBody")}
          </p>
        </CardContent>
      </Card>
    )
  }

  return (
    <div className="space-y-6">
      <ConfigPanel courseId={courseId} canSeed={false} />
      <ParticipantsPanel courseId={courseId} />
    </div>
  )
}
