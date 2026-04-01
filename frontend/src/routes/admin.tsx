import { createFileRoute, Outlet, useLocation, useNavigate } from "@tanstack/react-router"
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs"

export const Route = createFileRoute("/admin")({
  component: AdminLayout,
})

const TABS = [
  { value: "usage", label: "Platform Usage" },
  { value: "users", label: "User Management" },
] as const

const VALID_TABS = new Set(TABS.map((t) => t.value))

function AdminLayout() {
  const navigate = useNavigate()
  const location = useLocation()

  const lastSegment = location.pathname.split("/").pop() || ""
  const activeTab = VALID_TABS.has(lastSegment as typeof TABS[number]["value"]) ? lastSegment : "usage"

  return (
    <div className="space-y-6">
      <h2 className="text-2xl font-bold tracking-tight">Platform Admin</h2>

      <Tabs
        value={activeTab}
        onValueChange={(value: unknown) => {
          navigate({ to: `/admin/${value}` } as any)
        }}
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
