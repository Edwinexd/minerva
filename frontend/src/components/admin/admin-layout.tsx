import { Outlet, useLocation, useNavigate } from "@tanstack/react-router"
import { useQuery } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { userQuery } from "@/lib/queries"
import { useDocumentTitle } from "@/lib/use-document-title"
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"

const TAB_VALUES = [
  "usage",
  "users",
  "courses",
  "rules",
  "external-invites",
  "lti",
  "integrations",
  "study",
  "system",
] as const

type TabValue = (typeof TAB_VALUES)[number]

// Tabs an integrator (non-admin) may see. The integrator role exists to
// delegate exactly these two site-wide surfaces; every other admin tab stays
// admin-only (and its backend routes still return 403 for integrators).
const INTEGRATOR_TABS: readonly TabValue[] = ["lti", "integrations"]

const TAB_ROUTES = {
  usage: "/admin/usage",
  users: "/admin/users",
  courses: "/admin/courses",
  rules: "/admin/rules",
  "external-invites": "/admin/external-invites",
  lti: "/admin/lti",
  integrations: "/admin/integrations",
  study: "/admin/study",
  system: "/admin/system",
} as const satisfies Record<TabValue, string>

const TAB_LABEL_KEYS: Record<TabValue, string> = {
  usage: "layout.tabs.usage",
  users: "layout.tabs.users",
  courses: "layout.tabs.courses",
  rules: "layout.tabs.rules",
  "external-invites": "layout.tabs.externalInvites",
  lti: "layout.tabs.lti",
  integrations: "layout.tabs.integrations",
  study: "layout.tabs.study",
  system: "layout.tabs.system",
}

const TAB_TITLE_KEYS: Record<TabValue, string> = {
  usage: "pageTitles.adminTab.usage",
  users: "pageTitles.adminTab.users",
  courses: "pageTitles.adminTab.courses",
  rules: "pageTitles.adminTab.rules",
  "external-invites": "pageTitles.adminTab.externalInvites",
  lti: "pageTitles.adminTab.lti",
  integrations: "pageTitles.adminTab.integrations",
  study: "pageTitles.adminTab.study",
  system: "pageTitles.adminTab.system",
}

export function AdminLayout() {
  const navigate = useNavigate()
  const location = useLocation()
  const { t } = useTranslation("admin")
  const { t: tCommon } = useTranslation("common")
  const { data: user } = useQuery(userQuery)

  // Admins see every tab; integrators are limited to their two site-wide
  // surfaces. Anyone else shouldn't reach this layout (the nav entry and the
  // /admin redirect both gate on role), but fall back to no tabs to be safe.
  const visibleTabs: readonly TabValue[] =
    user?.role === "admin"
      ? TAB_VALUES
      : user?.role === "integrator"
        ? INTEGRATOR_TABS
        : []
  const visibleTabSet = new Set<TabValue>(visibleTabs)

  const lastSegment = location.pathname.split("/").pop() || ""
  const activeTab: TabValue = visibleTabSet.has(lastSegment as TabValue)
    ? (lastSegment as TabValue)
    : (visibleTabs[0] ?? "usage")

  useDocumentTitle(
    `${tCommon("pageTitles.admin")} – ${tCommon(TAB_TITLE_KEYS[activeTab])}`,
  )

  return (
    <div className="space-y-6">
      <h1 className="text-2xl font-bold tracking-tight">{t("layout.title")}</h1>

      <nav aria-label={t("layout.sectionNavLabel")}>
        <div className="md:hidden">
          <Select
            value={activeTab}
            onValueChange={(value) => {
              if (visibleTabSet.has(value as TabValue)) navigate({ to: TAB_ROUTES[value as TabValue] })
            }}
          >
            <SelectTrigger className="w-full" aria-label={t("layout.sectionNavLabel")}>
              <SelectValue>{t(TAB_LABEL_KEYS[activeTab])}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              {visibleTabs.map((value) => (
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
            if (visibleTabSet.has(value as TabValue)) navigate({ to: TAB_ROUTES[value as TabValue] })
          }}
          className="hidden md:flex"
        >
          <TabsList>
            {visibleTabs.map((value) => (
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
