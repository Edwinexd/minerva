import { createFileRoute, Link, Outlet, useLocation, useNavigate } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
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

const TABS = [
  { value: "config", label: "Configuration" },
  { value: "members", label: "Members" },
  { value: "conversations", label: "Conversations" },
  { value: "documents", label: "Documents" },
  { value: "invite", label: "Invite Links" },
  { value: "lti", label: "LTI" },
  { value: "api-keys", label: "API Keys" },
  { value: "play-designations", label: "Play Designations" },
  { value: "rag", label: "RAG Debug" },
  { value: "usage", label: "Usage" },
] as const

const VALID_TABS = new Set(TABS.map((t) => t.value))

function CourseEditPage() {
  const { courseId } = Route.useParams()
  const { data: course, isLoading } = useQuery(courseQuery(courseId))
  const navigate = useNavigate()
  const location = useLocation()

  const lastSegment = location.pathname.split("/").pop() || ""
  const activeTab = VALID_TABS.has(lastSegment as typeof TABS[number]["value"]) ? lastSegment : "config"

  if (isLoading) return (
    <div className="space-y-6">
      <Skeleton className="h-8 w-full max-w-64" />
      <Skeleton className="h-10 w-full max-w-80" />
      <Skeleton className="h-64 w-full" />
    </div>
  )
  if (!course) return <p className="text-muted-foreground">Course not found</p>

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h2 className="text-2xl font-bold tracking-tight">{course.name}</h2>
        <Link to="/course/$courseId" params={{ courseId }}>
          <Button variant="outline">Try Chat</Button>
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
              {TABS.find((t) => t.value === activeTab)?.label}
            </SelectValue>
          </SelectTrigger>
          <SelectContent>
            {TABS.map((tab) => (
              <SelectItem key={tab.value} value={tab.value}>
                {tab.label}
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
          {TABS.map((tab) => (
            <TabsTrigger key={tab.value} value={tab.value}>
              {tab.label}
            </TabsTrigger>
          ))}
        </TabsList>
      </Tabs>

      <Outlet />
    </div>
  )
}
