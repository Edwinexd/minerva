/**
 * Axe accessibility tests for the authenticated, data-driven pages that the
 * public-URL pa11y job can never reach (everything behind Shibboleth: the
 * chat surface, the teacher course tabs, and the admin panels).
 *
 * These render the real page components with:
 *   - a router stub (Link -> <a>, useNavigate/useLocation no-ops) so the
 *     components mount without a RouterProvider, and
 *   - a QueryClient pre-seeded with realistic fixtures so each page renders
 *     its loaded state (not a skeleton) for axe to inspect.
 *
 * `@/lib/api` is mocked to a never-resolving stub: seeded queries render from
 * cache immediately, and any background refetch / mutation stays pending
 * instead of hitting the network.
 */
import type { ReactElement } from "react"
import { createElement } from "react"
import { describe, expect, it, vi } from "vitest"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { I18nextProvider } from "react-i18next"
import { render } from "@testing-library/react"
import i18n from "i18next"

import "@/i18n"
import { axe } from "./a11y"

// ── Module mocks ────────────────────────────────────────────────────────

vi.mock("@tanstack/react-router", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@tanstack/react-router")>()
  // Router-only props that must not leak onto the DOM <a>.
  const ROUTER_PROPS = new Set([
    "params", "search", "hash", "state", "mask", "from", "replace",
    "resetScroll", "preload", "preloadDelay", "activeProps", "inactiveProps",
    "activeOptions", "startTransition", "viewTransition",
  ])
  const Link = ({
    to,
    children,
    ...rest
  }: { to?: unknown; children?: React.ReactNode } & Record<string, unknown>) => {
    const domProps = Object.fromEntries(
      Object.entries(rest).filter(([k]) => !ROUTER_PROPS.has(k)),
    )
    return createElement(
      "a",
      { href: typeof to === "string" ? to : "#", ...domProps },
      children,
    )
  }
  return {
    ...actual,
    Link,
    useNavigate: () => () => {},
    useLocation: () => ({ pathname: "/", search: "", hash: "" }),
    useRouter: () => ({ navigate: () => {} }),
  }
})

vi.mock("@/lib/api", () => {
  const pending = () => new Promise(() => {})
  return {
    api: {
      get: vi.fn(pending),
      post: vi.fn(pending),
      put: vi.fn(pending),
      delete: vi.fn(pending),
    },
  }
})

// ── Imports that depend on the mocks above ──────────────────────────────

import * as queries from "@/lib/queries"
import type {
  AdminUser,
  Course,
  CourseMember,
  Document,
  User,
} from "@/lib/types"
import { UserManagementPanel } from "@/components/admin/users-page"
import { ConfigPage } from "@/components/teacher/config-page"
import { DocumentsPage } from "@/components/teacher/documents-page"
import { MembersPage } from "@/components/teacher/members-page"
import { ConversationsPage } from "@/components/teacher/conversations-page"
import { NewChatRouteComponent } from "@/components/chat/chat-page"
import { TeacherHelpPage } from "@/components/teacher-help-page"
import { CourseManagementPanel } from "@/components/admin/courses-page"
import { DaisyImportsPanel } from "@/components/admin/daisy-imports-page"
import { AdminDefaultsPanel } from "@/components/admin/defaults-page"
import { RoleRulesPanel } from "@/components/admin/rules-page"

// ── Fixtures ────────────────────────────────────────────────────────────

const COURSE_ID = "course-1"
const useParams = () => ({ courseId: COURSE_ID })

const user: User = {
  id: "user-1",
  eppn: "teacher@su.se",
  display_name: "Teacher One",
  role: "teacher",
  privacy_acknowledged_at: "2026-01-01T00:00:00Z",
}

const course: Course = {
  id: COURSE_ID,
  name: "Programming 1",
  description: "Intro course",
  owner_id: "user-1",
  context_ratio: 0.5,
  temperature: 0.7,
  model: "llama-3.3-70b",
  system_prompt: null,
  max_chunks: 8,
  min_score: 0.3,
  strategy: "simple",
  tool_use_enabled: false,
  embedding_provider: "local",
  embedding_model: "bge-small",
  embedding_version: 1,
  reranker_model: "jinaai/jina-reranker-v2-base-multilingual",
  daily_cost_limit_usd: 0.5,
  active: true,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  my_role: "teacher",
  feature_flags: {
    course_kg: false,
    aegis: false,
    concept_graph: false,
  },
  semester_label: null,
  daisy_offerings: [],
  auto_managed: false,
  course_code: null,
  student_count: 30,
}

const adminUsers: AdminUser[] = [
  {
    id: "user-1",
    eppn: "teacher@su.se",
    display_name: "Teacher One",
    role: "teacher",
    suspended: false,
    role_manually_set: false,
    owner_daily_cost_limit_usd: 0,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  },
  {
    id: "user-2",
    eppn: "student@su.se",
    display_name: null,
    role: "student",
    suspended: true,
    role_manually_set: true,
    owner_daily_cost_limit_usd: 2.5,
    created_at: "2026-01-02T00:00:00Z",
    updated_at: "2026-01-02T00:00:00Z",
  },
]

const members: CourseMember[] = [
  {
    user_id: "user-2",
    eppn: "student@su.se",
    display_name: "Student Two",
    role: "student",
    added_at: "2026-01-02T00:00:00Z",
  },
]

const documents: Document[] = [
  {
    id: "doc-1",
    course_id: COURSE_ID,
    filename: "lecture-1.pdf",
    mime_type: "application/pdf",
    size_bytes: 12345,
    status: "ready",
    chunk_count: 10,
    error_msg: null,
    displayable: true,
    uploaded_by: "user-1",
    created_at: "2026-01-03T00:00:00Z",
    processed_at: "2026-01-03T00:01:00Z",
    kind: null,
    kind_confidence: null,
    kind_rationale: null,
    kind_locked_by_teacher: false,
    classified_at: null,
    source_system: null,
    source_ref: null,
    orphaned_at: null,
  },
]

const archivedCourse: Course = {
  ...course,
  id: "course-2",
  name: "Programming 2",
  active: false,
  course_code: "PROG2",
  semester_label: "VT2026",
}

const daisyPending = {
  auto_apply: false,
  auto_apply_updated_at: "2026-01-04T00:00:00Z",
  auto_apply_updated_by: "teacher@su.se",
  pending: [
    {
      id: "daisy-1",
      momenttillf_id: "12345",
      course_code: "PROG1",
      name: "Programming 1 VT2026",
      semester_label: "VT2026",
      daisy_info_url: null,
      daisy_syllabus_url: null,
      daisy_unit: "DSV",
      participant_count: 1,
      participants: [
        {
          display_name: "Student Two",
          eppns: ["student@su.se"],
          daisy_roles: ["student"],
          kind: "student",
        },
      ],
      existing_course_id: null,
      diff: {
        is_new_course: true,
        metadata_changes: [],
        member_changes: [],
      },
      first_seen_at: "2026-01-04T00:00:00Z",
      last_seen_at: "2026-01-05T00:00:00Z",
    },
  ],
}

// One knob per widget kind, so the axe pass covers every control the
// Defaults tab can render rather than just the first one.
const systemDefaults = {
  defaults: [
    {
      key: "course.temperature",
      category: "course_ai" as const,
      label_key: "defaults.course.temperature.label",
      description_key: "defaults.course.temperature.description",
      kind: { type: "float" as const, min: 0, max: 1 },
      env_var: null,
      fallback: 0.3,
      value: 0.7,
      has_row: true,
      updated_at: "2026-01-06T00:00:00Z",
    },
    {
      key: "course.maxChunks",
      category: "course_ai" as const,
      label_key: "defaults.course.maxChunks.label",
      description_key: "defaults.course.maxChunks.description",
      kind: { type: "int" as const, min: 1, max: 50 },
      env_var: null,
      fallback: 10,
      value: 10,
      has_row: false,
      updated_at: null,
    },
    {
      key: "platform.ownerDailyCostLimitUsd",
      category: "platform" as const,
      label_key: "defaults.platform.ownerDailyCostLimitUsd.label",
      description_key: "defaults.platform.ownerDailyCostLimitUsd.description",
      kind: { type: "float" as const, min: 0, max: 1000 },
      env_var: "MINERVA_DEFAULT_OWNER_DAILY_COST_LIMIT_USD",
      fallback: 5,
      value: 5,
      has_row: false,
      updated_at: null,
    },
  ],
}

const roleRules = [
  {
    id: "rule-1",
    name: "DSV staff to teacher",
    target_role: "teacher" as const,
    enabled: true,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    conditions: [
      {
        id: "cond-1",
        rule_id: "rule-1",
        attribute: "affiliation",
        operator: "contains",
        value: "employee@su.se",
      },
    ],
  },
]

// ── Harness ─────────────────────────────────────────────────────────────

type Seed = [readonly unknown[], unknown]

function renderPage(ui: ReactElement, seeds: Seed[]) {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, refetchOnWindowFocus: false },
      mutations: { retry: false },
    },
  })
  for (const [key, data] of seeds) queryClient.setQueryData(key, data)
  return render(
    createElement(
      I18nextProvider,
      { i18n },
      createElement(QueryClientProvider, { client: queryClient }, ui),
    ),
  )
}

// ── Tests ───────────────────────────────────────────────────────────────

describe("Authenticated pages a11y", () => {
  it("admin user management has no axe violations", async () => {
    const { container, getByText } = renderPage(<UserManagementPanel />, [
      [queries.adminUsersQuery.queryKey, adminUsers],
    ])
    // Confirm the loaded table rendered (not a skeleton) so the axe check
    // is meaningful.
    expect(getByText("student@su.se")).toBeInTheDocument()
    expect(await axe(container)).toHaveNoViolations()
  })

  it("teacher config page has no axe violations", async () => {
    const { container, getByDisplayValue } = renderPage(
      <ConfigPage useParams={useParams} />,
      [
        [queries.courseQuery(COURSE_ID).queryKey, course],
        [queries.courseKgTokenUsageQuery(COURSE_ID).queryKey, { since: "2026-01-01T00:00:00Z", rows: [] }],
        [queries.modelsQuery.queryKey, { models: [{ id: "llama-3.3-70b", name: "Llama 3.3 70B" }] }],
        [queries.embeddingModelsQuery.queryKey, { models: [{ model: "bge-small", dimensions: 384, benchmark: null }] }],
      ],
    )
    expect(getByDisplayValue("Programming 1")).toBeInTheDocument()
    expect(await axe(container)).toHaveNoViolations()
  })

  it("teacher documents page has no axe violations", async () => {
    const { container, getByText } = renderPage(
      <DocumentsPage useParams={useParams} />,
      [
        [queries.courseQuery(COURSE_ID).queryKey, course],
        [queries.courseDocumentsQuery(COURSE_ID).queryKey, documents],
      ],
    )
    expect(getByText("lecture-1.pdf")).toBeInTheDocument()
    expect(await axe(container)).toHaveNoViolations()
  })

  it("teacher members page has no axe violations", async () => {
    const { container, getByText } = renderPage(
      <MembersPage useParams={useParams} />,
      [
        [queries.courseQuery(COURSE_ID).queryKey, course],
        [queries.courseMembersQuery(COURSE_ID).queryKey, members],
        [queries.courseRoleSuggestionsQuery(COURSE_ID).queryKey, []],
      ],
    )
    expect(getByText("Student Two")).toBeInTheDocument()
    expect(await axe(container)).toHaveNoViolations()
  })

  it("student new-chat page has no axe violations", async () => {
    const { container, getByText } = renderPage(
      <NewChatRouteComponent useParams={useParams} />,
      [
        [queries.courseQuery(COURSE_ID).queryKey, course],
        [queries.conversationsQuery(COURSE_ID).queryKey, []],
        [queries.pinnedConversationsQuery(COURSE_ID).queryKey, []],
        [queries.suggestedQuestionsQuery(COURSE_ID).queryKey, { questions: ["What is recursion?"] }],
        [queries.userQuery.queryKey, user],
      ],
    )
    expect(getByText("What is recursion?")).toBeInTheDocument()
    expect(await axe(container)).toHaveNoViolations()
  })

  it("teacher conversation themes have no axe violations", async () => {
    const { container, getByText } = renderPage(
      <ConversationsPage useParams={useParams} />,
      [
        [queries.allConversationsQuery(COURSE_ID).queryKey, []],
        [
          queries.popularTopicsQuery(COURSE_ID).queryKey,
          [
            {
              topic: "Data storage architecture",
              summary: "Students need clarification on how course data is persisted.",
              conversation_count: 3,
              unique_users: 2,
              total_messages: 8,
              conversation_ids: ["conversation-1", "conversation-2", "conversation-3"],
            },
          ],
        ],
        [
          queries.courseFeedbackStatsQuery(COURSE_ID).queryKey,
          { total_up: 0, total_down: 0, categories: [] },
        ],
        [queries.conversationFlagKindsQuery(COURSE_ID).queryKey, {}],
      ],
    )
    expect(getByText("Data storage architecture")).toBeInTheDocument()
    expect(await axe(container)).toHaveNoViolations()
  })

  it("teacher help page has no axe violations", async () => {
    const { container, getByText } = renderPage(<TeacherHelpPage />, [])
    expect(getByText("Set up Minerva for your course")).toBeInTheDocument()
    expect(await axe(container)).toHaveNoViolations()
  })

  it("admin course management has no axe violations", async () => {
    const { container, getByText } = renderPage(<CourseManagementPanel />, [
      [queries.adminCoursesQuery.queryKey, [course, archivedCourse]],
      [queries.adminUsersQuery.queryKey, adminUsers],
      [queries.adminMergeSuggestionsQuery.queryKey, []],
    ])
    expect(getByText("Programming 2")).toBeInTheDocument()
    expect(await axe(container)).toHaveNoViolations()
  })

  it("admin course management has no axe violations with rows selected", async () => {
    // The bulk action bar only mounts once something is ticked, so the
    // default render never sees it.
    const { container, getByText, getAllByRole } = renderPage(
      <CourseManagementPanel />,
      [
        [queries.adminCoursesQuery.queryKey, [course, archivedCourse]],
        [queries.adminUsersQuery.queryKey, adminUsers],
        [queries.adminMergeSuggestionsQuery.queryKey, []],
      ],
    )
    // First checkbox is the header select-all.
    getAllByRole("checkbox")[0].click()
    expect(getByText("Edit settings")).toBeInTheDocument()
    expect(await axe(container)).toHaveNoViolations()
  })

  it("admin Daisy imports has no axe violations", async () => {
    const { container, getByText } = renderPage(<DaisyImportsPanel />, [
      [queries.daisyPendingQuery.queryKey, daisyPending],
    ])
    expect(getByText("Programming 1 VT2026")).toBeInTheDocument()
    expect(await axe(container)).toHaveNoViolations()
  })

  it("admin defaults has no axe violations", async () => {
    const { container, getByText } = renderPage(<AdminDefaultsPanel />, [
      [["admin", "system-defaults"], systemDefaults],
    ])
    expect(getByText("Default temperature")).toBeInTheDocument()
    expect(await axe(container)).toHaveNoViolations()
  })

  it("admin role rules has no axe violations", async () => {
    const { container, getByText } = renderPage(<RoleRulesPanel />, [
      [queries.adminRoleRulesQuery.queryKey, roleRules],
      [
        queries.adminRoleRuleAttributeValuesQuery.queryKey,
        { by_attribute: {}, min_users: 2 },
      ],
    ])
    expect(getByText("DSV staff to teacher")).toBeInTheDocument()
    expect(await axe(container)).toHaveNoViolations()
  })
})
