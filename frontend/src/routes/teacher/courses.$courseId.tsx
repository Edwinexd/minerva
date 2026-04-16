import { createFileRoute, Link, Outlet, useLocation, useNavigate } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { courseQuery } from "@/lib/queries"
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

export const Route = createFileRoute("/teacher/courses/$courseId")({
  component: CourseEditPage,
})

const TAB_VALUES = [
  "config",
  "members",
  "conversations",
  "documents",
  "invite",
  "lti",
  "canvas",
  "api-keys",
  "play-designations",
  "rag",
  "usage",
] as const

const TAB_LABEL_KEYS: Record<(typeof TAB_VALUES)[number], string> = {
  "config": "layout.tabs.config",
  "members": "layout.tabs.members",
  "conversations": "layout.tabs.conversations",
  "documents": "layout.tabs.documents",
  "invite": "layout.tabs.invite",
  "lti": "layout.tabs.lti",
  "canvas": "layout.tabs.canvas",
  "api-keys": "layout.tabs.apiKeys",
  "play-designations": "layout.tabs.playDesignations",
  "rag": "layout.tabs.rag",
  "usage": "layout.tabs.usage",
}

// Tabs that TAs cannot see: invite/LTI/API keys/play designations are
// teacher-only operations enforced server-side; hide them in the UI too.
const TA_HIDDEN_TABS = new Set<string>([
  "invite",
  "lti",
  "canvas",
  "api-keys",
  "play-designations",
])

function CourseEditPage() {
  const { courseId } = Route.useParams()
  const { data: course, isLoading } = useQuery(courseQuery(courseId))
  const navigate = useNavigate()
  const location = useLocation()
  const { t } = useTranslation("teacher")

  const visibleTabValues = course?.my_role === "ta"
    ? TAB_VALUES.filter((v) => !TA_HIDDEN_TABS.has(v))
    : TAB_VALUES
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
            if (value) navigate({ to: `/teacher/courses/${courseId}/${value}` } as any)
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
          navigate({ to: `/teacher/courses/${courseId}/${value}` } as any)
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
