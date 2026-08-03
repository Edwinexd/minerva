/**
 * Focus management for the Aegis rail on the chat surface.
 *
 * Below `aegisDrawerBreakpoint` the panel is a fixed drawer over the
 * chat with a dismiss backdrop, so hiding and re-showing it without
 * moving focus leaves keyboard users somewhere they cannot see. The
 * panel is not modal above the breakpoint, so this is a disclosure
 * (focus in on open, back to the trigger on close), not a focus trap.
 *
 * The page-load case is the one to keep honest: `panelVisible` is
 * storage-backed and defaults to true, so focus must NOT move on the
 * initial render.
 */
import { createElement } from "react"
import { beforeEach, describe, expect, it, vi } from "vitest"
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

import * as queries from "@/lib/queries"
import type { Course, User } from "@/lib/types"
import { NewChatRouteComponent } from "@/components/chat/chat-page"

const COURSE_ID = "course-1"

const course: Course = {
  id: COURSE_ID,
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
  active: true,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  my_role: "student",
  feature_flags: { course_kg: false, aegis: true, concept_graph: false },
  semester_label: null,
  daisy_offerings: [],
  auto_managed: false,
  course_code: null,
  student_count: 30,
}

const user: User = {
  id: "user-2",
  eppn: "student@su.se",
  display_name: "Student Two",
  role: "student",
  privacy_acknowledged_at: "2026-01-01T00:00:00Z",
}

function renderChat() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, refetchOnWindowFocus: false },
      mutations: { retry: false },
    },
  })
  queryClient.setQueryData(queries.courseQuery(COURSE_ID).queryKey, course)
  queryClient.setQueryData(queries.conversationsQuery(COURSE_ID).queryKey, [])
  queryClient.setQueryData(
    queries.pinnedConversationsQuery(COURSE_ID).queryKey,
    [],
  )
  queryClient.setQueryData(
    queries.suggestedQuestionsQuery(COURSE_ID).queryKey,
    { questions: [] },
  )
  queryClient.setQueryData(queries.userQuery.queryKey, user)
  return render(
    createElement(
      I18nextProvider,
      { i18n },
      createElement(
        QueryClientProvider,
        { client: queryClient },
        createElement(NewChatRouteComponent, {
          useParams: () => ({ courseId: COURSE_ID }),
        }),
      ),
    ),
  )
}

describe("Aegis panel focus", () => {
  // Visibility is storage-backed, so a test that leaves the panel
  // closed would otherwise decide the next test's starting state.
  beforeEach(() => {
    try {
      window.localStorage.removeItem("minerva.aegis.panel.visible")
    } catch {
      // Storage is optional in this environment; the hook defaults to
      // visible when it cannot read one.
    }
  })

  it("does not steal focus on first render", () => {
    renderChat()
    expect(screen.getByRole("complementary", { name: "Aegis panel" }))
      .toBeInTheDocument()
    expect(document.activeElement).toBe(document.body)
  })

  it("returns focus to the pill on close and back into the panel on reopen", async () => {
    const user = userEvent.setup()
    renderChat()

    const panel = screen.getByRole("complementary", { name: "Aegis panel" })

    await user.click(screen.getByRole("button", { name: "Hide Aegis panel" }))
    const pill = await screen.findByRole("button", {
      name: "Show Aegis panel",
    })
    await waitFor(() => {
      expect(document.activeElement).toBe(pill)
    })

    await user.click(pill)
    await waitFor(() => {
      expect(document.activeElement).toBe(
        screen.getByRole("complementary", { name: "Aegis panel" }),
      )
    })
    expect(panel).toBeDefined()
  })

  it("closes on Escape while focus is inside the panel", async () => {
    const user = userEvent.setup()
    renderChat()

    // Open via the pill so focus starts inside the panel.
    await user.click(screen.getByRole("button", { name: "Hide Aegis panel" }))
    await user.click(screen.getByRole("button", { name: "Show Aegis panel" }))
    await waitFor(() => {
      expect(document.activeElement).toBe(
        screen.getByRole("complementary", { name: "Aegis panel" }),
      )
    })

    await user.keyboard("{Escape}")

    await waitFor(() => {
      expect(
        screen.queryByRole("complementary", { name: "Aegis panel" }),
      ).not.toBeInTheDocument()
    })
  })
})
