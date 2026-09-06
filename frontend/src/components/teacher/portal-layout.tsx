import { Outlet, useLocation, useNavigate } from "@tanstack/react-router"
import { useTranslation } from "react-i18next"
import { useDocumentTitle } from "@/lib/use-document-title"
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"

/**
 * Shell for the teacher-wide surfaces: the things that belong to the
 * teacher rather than to one course. Mirrors `AdminLayout`, including
 * the mobile Select / desktop Tabs pair.
 *
 * It is a pathless route (`teacher/_portal`) so it wraps only these
 * pages; `/teacher/courses/$courseId/*` keeps its own course-scoped tab
 * bar and stays outside this shell.
 */
const TAB_VALUES = ["usage", "guide"] as const

type TabValue = (typeof TAB_VALUES)[number]

const TAB_ROUTES = {
  usage: "/teacher/usage",
  guide: "/teacher/guide",
} as const satisfies Record<TabValue, string>

const TAB_LABEL_KEYS: Record<TabValue, string> = {
  usage: "portal.tabs.usage",
  guide: "portal.tabs.guide",
}

const TAB_TITLE_KEYS: Record<TabValue, string> = {
  usage: "pageTitles.teacherTab.usage",
  guide: "pageTitles.teacherTab.guide",
}

export function TeacherPortalLayout() {
  const navigate = useNavigate()
  const location = useLocation()
  const { t } = useTranslation("teacher")
  const { t: tCommon } = useTranslation("common")

  const lastSegment = location.pathname.split("/").pop() || ""
  const activeTab: TabValue = TAB_VALUES.includes(lastSegment as TabValue)
    ? (lastSegment as TabValue)
    : "usage"

  useDocumentTitle(
    `${tCommon("pageTitles.teacher")} – ${tCommon(TAB_TITLE_KEYS[activeTab])}`,
  )

  return (
    <div className="space-y-6">
      <h1 className="text-2xl font-bold tracking-tight">{t("portal.title")}</h1>

      <nav aria-label={t("portal.sectionNavLabel")}>
        <div className="md:hidden">
          <Select
            value={activeTab}
            onValueChange={(value) => {
              if (TAB_VALUES.includes(value as TabValue)) navigate({ to: TAB_ROUTES[value as TabValue] })
            }}
          >
            <SelectTrigger className="w-full" aria-label={t("portal.sectionNavLabel")}>
              <SelectValue>{t(TAB_LABEL_KEYS[activeTab])}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              {TAB_VALUES.map((value) => (
                <SelectItem key={value} value={value}>
                  {t(TAB_LABEL_KEYS[value])}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <Tabs
          value={activeTab}
          onValueChange={(value: unknown) => {
            if (TAB_VALUES.includes(value as TabValue)) navigate({ to: TAB_ROUTES[value as TabValue] })
          }}
          className="hidden md:flex"
        >
          <TabsList>
            {TAB_VALUES.map((value) => (
              <TabsTrigger key={value} value={value}>
                {t(TAB_LABEL_KEYS[value])}
              </TabsTrigger>
            ))}
          </TabsList>
        </Tabs>
      </nav>

      <Outlet />
    </div>
  )
}
