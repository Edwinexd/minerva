/**
 * Regression test for the admin bulk-course actions.
 *
 * A fully-successful bulk archive clears the selection, which unmounts
 * the bulk action bar. When the result summary lived inside that bar it
 * was destroyed in the same commit (no confirmation for anyone) and the
 * dialog was destroyed rather than closed, so its focus restore never
 * ran and focus fell to <body>. The summary is owned by the panel now
 * and claims focus on mount; this test pins both halves of that.
 */
import { createElement } from "react"
import { describe, expect, it, vi } from "vitest"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { I18nextProvider } from "react-i18next"
import { render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import i18n from "i18next"

import "@/i18n"

vi.mock("@tanstack/react-router", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@tanstack/react-router")>()
  const Link = ({
    children,
  }: {
    children?: React.ReactNode
  } & Record<string, unknown>) => createElement("a", { href: "#" }, children)
  return { ...actual, Link, useNavigate: () => () => {} }
})

const bulkResponse = {
  succeeded: 1,
  failed: 0,
  results: [{ course_id: "course-1", ok: true, error: null }],
}

vi.mock("@/lib/api", () => {
  const pending = () => new Promise(() => {})
  return {
    api: {
      get: vi.fn(pending),
      post: vi.fn(() => Promise.resolve(bulkResponse)),
      put: vi.fn(pending),
      delete: vi.fn(pending),
    },
  }
})

import * as queries from "@/lib/queries"
import type { Course } from "@/lib/types"
import { CourseManagementPanel } from "@/components/admin/courses-page"

const course: Course = {
  id: "course-1",
  name: "Programming 1",
  description: null,
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
  conversation_soft_token_limit: 300000,
  conversation_hard_token_limit: 1000000,
  active: true,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  my_role: "teacher",
  feature_flags: {
    course_kg: false,
    aegis: false,
    concept_graph: false,
    conversation_limits: false,
  },
  semester_label: null,
  daisy_offerings: [],
  auto_managed: false,
  course_code: null,
  student_count: 30,
}

function renderPanel() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, refetchOnWindowFocus: false },
      mutations: { retry: false },
    },
  })
  queryClient.setQueryData(queries.adminCoursesQuery.queryKey, [course])
  queryClient.setQueryData(queries.adminUsersQuery.queryKey, [])
  queryClient.setQueryData(queries.adminMergeSuggestionsQuery.queryKey, [])
  return render(
    createElement(
      I18nextProvider,
      { i18n },
      createElement(
        QueryClientProvider,
        { client: queryClient },
        createElement(CourseManagementPanel),
      ),
    ),
  )
}

describe("admin bulk course actions", () => {
  it("keeps the result summary and focus after a fully-successful archive", async () => {
    const user = userEvent.setup()
    renderPanel()

    await user.click(screen.getByLabelText("Select Programming 1"))
    await user.click(screen.getByRole("button", { name: "Archive (1)" }))
    await user.click(screen.getByRole("button", { name: "Confirm" }))

    // Bar is gone (selection cleared) but the summary outlives it.
    const summary = await screen.findByText("1 updated, 0 failed.")
    expect(summary).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Edit settings" }),
    ).not.toBeInTheDocument()

    // <output> is an implicit live region, so the summary announces.
    const live = summary.closest("output")
    expect(live).not.toBeNull()

    // ...and the dialog handed focus to it instead of dropping to <body>.
    await waitFor(() => {
      expect(document.activeElement).toBe(live)
    })
  })
})
