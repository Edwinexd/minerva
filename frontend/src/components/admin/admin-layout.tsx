import { Outlet, useLocation, useNavigate } from "@tanstack/react-router"
import { useTranslation } from "react-i18next"
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
  "system",
] as const

type TabValue = (typeof TAB_VALUES)[number]

const VALID_TABS = new Set<TabValue>(TAB_VALUES)

const TAB_ROUTES = {
  usage: "/admin/usage",
  users: "/admin/users",
  courses: "/admin/courses",
  rules: "/admin/rules",
  "external-invites": "/admin/external-invites",
  lti: "/admin/lti",
  system: "/admin/system",
} as const satisfies Record<TabValue, string>

const TAB_LABEL_KEYS: Record<TabValue, string> = {
  usage: "layout.tabs.usage",
  users: "layout.tabs.users",
  courses: "layout.tabs.courses",
  rules: "layout.tabs.rules",
  "external-invites": "layout.tabs.externalInvites",
  lti: "layout.tabs.lti",
  system: "layout.tabs.system",
}

export function AdminLayout() {
  const navigate = useNavigate()
  const location = useLocation()
  const { t } = useTranslation("admin")

  const lastSegment = location.pathname.split("/").pop() || ""
  const activeTab: TabValue = VALID_TABS.has(lastSegment as TabValue)
    ? (lastSegment as TabValue)
    : "usage"

  return (
    <div className="space-y-6">
      <h2 className="text-2xl font-bold tracking-tight">{t("layout.title")}</h2>

      <div className="md:hidden">
        <Select
          value={activeTab}
          onValueChange={(value) => {
            if (VALID_TABS.has(value as TabValue)) navigate({ to: TAB_ROUTES[value as TabValue] })
          }}
        >
          <SelectTrigger className="w-full">
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
          if (VALID_TABS.has(value as TabValue)) navigate({ to: TAB_ROUTES[value as TabValue] })
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

      <Outlet />
    </div>
  )
}
