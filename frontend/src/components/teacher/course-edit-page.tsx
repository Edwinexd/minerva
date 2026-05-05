import { Link, Outlet, useLocation, useNavigate } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { courseQuery } from "@/lib/queries"
import { useDocumentTitle } from "@/lib/use-document-title"
import { Button } from "@/components/ui/button"
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Skeleton } from "@/components/ui/skeleton"

const TAB_VALUES = [
  "config",
  "members",
  "conversations",
  "documents",
  "knowledge-graph",
  "invite",
  "lti",
  "canvas",
  "api-keys",
  "play-designations",
  "rag",
  "study",
  "usage",
] as const

const TAB_LABEL_KEYS: Record<(typeof TAB_VALUES)[number], string> = {
  "config": "layout.tabs.config",
  "members": "layout.tabs.members",
  "conversations": "layout.tabs.conversations",
  "documents": "layout.tabs.documents",
  "knowledge-graph": "layout.tabs.knowledgeGraph",
  "invite": "layout.tabs.invite",
  "lti": "layout.tabs.lti",
  "canvas": "layout.tabs.canvas",
  "api-keys": "layout.tabs.apiKeys",
  "play-designations": "layout.tabs.playDesignations",
  "rag": "layout.tabs.rag",
  "study": "layout.tabs.study",
  "usage": "layout.tabs.usage",
}

type TabValue = (typeof TAB_VALUES)[number]

const TAB_ROUTES = {
  "config": "/teacher/courses/$courseId/config",
  "members": "/teacher/courses/$courseId/members",
  "conversations": "/teacher/courses/$courseId/conversations",
  "documents": "/teacher/courses/$courseId/documents",
  "knowledge-graph": "/teacher/courses/$courseId/knowledge-graph",
  "invite": "/teacher/courses/$courseId/invite",
  "lti": "/teacher/courses/$courseId/lti",
  "canvas": "/teacher/courses/$courseId/canvas",
  "api-keys": "/teacher/courses/$courseId/api-keys",
  "play-designations": "/teacher/courses/$courseId/play-designations",
  "rag": "/teacher/courses/$courseId/rag",
  "study": "/teacher/courses/$courseId/study",
  "usage": "/teacher/courses/$courseId/usage",
} as const satisfies Record<TabValue, string>

// Tabs that TAs cannot see: invite/LTI/API keys/play designations are
// teacher-only operations enforced server-side; hide them in the UI too.
// `study` is also teacher-only (study config + per-participant data).
const TA_HIDDEN_TABS = new Set<string>([
  "invite",
  "lti",
  "canvas",
  "api-keys",
  "play-designations",
  "study",
])

export function CourseEditPage({ useParams }: { useParams: () => { courseId: string } }) {
  const { courseId } = useParams()
  const { data: course, isLoading } = useQuery(courseQuery(courseId))
  const navigate = useNavigate()
  const location = useLocation()
  const { t } = useTranslation("teacher")
  const { t: tCommon } = useTranslation("common")

  useDocumentTitle(course ? tCommon("pageTitles.teacherCourse", { course: course.name }) : null)

  // Tab visibility is the union of role gating and feature-flag
  // gating. KG-only tabs (knowledge-graph today) hide automatically
  // when the course doesn't have the `course_kg` flag flipped on by
  // an admin; matches the backend, which 404s those endpoints in
  // the same case.
  const kgEnabled = course?.feature_flags?.course_kg === true
  const studyEnabled = course?.feature_flags?.study_mode === true
  const baseTabs = course?.my_role === "ta"
    ? TAB_VALUES.filter((v) => !TA_HIDDEN_TABS.has(v))
    : TAB_VALUES
  const visibleTabValues = baseTabs
    .filter((v) => kgEnabled || v !== "knowledge-graph")
    // Study tab is per-course feature-gated AND TA-hidden; surveys
    // and per-participant transcripts are sensitive enough that
    // "teacher" is the right floor (matches the backend gate in
    // `routes::study::require_course_owner_teacher_or_admin`).
    .filter((v) => studyEnabled || v !== "study")
    .filter((v) => course?.my_role !== "ta" || v !== "study")
  const validTabs = new Set<string>(visibleTabValues)

  const lastSegment = location.pathname.split("/").pop() || ""
  const activeTab = validTabs.has(lastSegment) ? lastSegment : "config"

  if (isLoading) return (
    <div className="space-y-6">
      <Skeleton className="h-8 w-full max-w-64" />
      <Skeleton className="h-10 w-full max-w-80" />
      <Skeleton className="h-64 w-full" />
    </div>
  )
  if (!course) return <p className="text-muted-foreground">{t("layout.courseNotFound")}</p>

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h2 className="text-2xl font-bold tracking-tight">{course.name}</h2>
        <Link to="/course/$courseId" params={{ courseId }}>
          <Button variant="outline">{t("layout.tryChat")}</Button>
        </Link>
      </div>

      <div className="md:hidden">
        <Select
          value={activeTab}
          onValueChange={(value) => {
            if (typeof value === "string" && Object.hasOwn(TAB_ROUTES, value)) navigate({ to: TAB_ROUTES[value as TabValue], params: { courseId } })
          }}
        >
          <SelectTrigger className="w-full">
            <SelectValue>
              {visibleTabValues.includes(activeTab as (typeof TAB_VALUES)[number])
                ? t(TAB_LABEL_KEYS[activeTab as (typeof TAB_VALUES)[number]])
                : null}
            </SelectValue>
          </SelectTrigger>
          <SelectContent>
            {visibleTabValues.map((tab) => (
              <SelectItem key={tab} value={tab}>
                {t(TAB_LABEL_KEYS[tab])}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <Tabs
        value={activeTab}
        onValueChange={(value: unknown) => {
          if (typeof value === "string" && Object.hasOwn(TAB_ROUTES, value)) navigate({ to: TAB_ROUTES[value as TabValue], params: { courseId } })
        }}
        className="hidden md:flex"
      >
        <TabsList>
          {visibleTabValues.map((tab) => (
            <TabsTrigger key={tab} value={tab}>
              {t(TAB_LABEL_KEYS[tab])}
            </TabsTrigger>
          ))}
        </TabsList>
      </Tabs>

      <Outlet />
    </div>
  )
}
