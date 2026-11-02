/**
 * Regression test for the suggested merges panel.
 *
 * Merging a group removes it from the list. The next group must arrive with
 * its own survivor: inheriting the previous pick hid the "keep" badge and
 * sent every course in the group to a course outside it.
 */
import { createElement } from "react"
import { beforeAll, describe, expect, it, vi } from "vitest"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { I18nextProvider } from "react-i18next"
import { act, render, screen, waitFor } from "@testing-library/react"
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
      post: vi.fn(() => Promise.resolve({})),
      put: vi.fn(pending),
      delete: vi.fn(pending),
    },
  }
})

import { api } from "@/lib/api"
import * as queries from "@/lib/queries"
import type { MergeSuggestionGroup } from "@/lib/types"
import { CourseManagementPanel } from "@/components/admin/courses-page"

const group = (code: string): MergeSuggestionGroup => ({
  code,
  semester_label: "HT2026",
  courses: ["a", "b"].map((suffix) => ({
    id: `${code}-${suffix}`,
    name: `${code} ${suffix}`,
    course_code: code,
    semester_label: "HT2026",
    owner_id: "user-1",
    auto_managed: false,
  })),
})

describe("suggested merges", () => {
  beforeAll(() => i18n.changeLanguage("en"))

  it("gives the next group its own survivor after a merge", async () => {
    const queryClient = new QueryClient({
      defaultOptions: {
        queries: { retry: false, refetchOnWindowFocus: false },
        mutations: { retry: false },
      },
    })
    const key = queries.adminMergeSuggestionsQuery.queryKey
    queryClient.setQueryData(queries.adminCoursesQuery.queryKey, [])
    queryClient.setQueryData(queries.adminUsersQuery.queryKey, [])
    queryClient.setQueryData(key, [group("FIRST"), group("SECOND")])
    render(
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
    await waitFor(() => expect(screen.getAllByText("keep")).toHaveLength(2))

    act(() => queryClient.setQueryData(key, [group("SECOND")]))
    await waitFor(() => expect(screen.getAllByText("keep")).toHaveLength(1))

    const user = userEvent.setup()
    await user.click(
      screen.getByRole("button", {
        name: i18n.t("courses.suggestions.mergeButton", { ns: "admin" }),
      }),
    )
    await user.click(
      await screen.findByRole("button", {
        name: i18n.t("courses.suggestions.confirmButton", { ns: "admin" }),
      }),
    )
    await waitFor(() =>
      expect(api.post).toHaveBeenCalledWith("/admin/courses/merge", {
        survivor_id: "SECOND-a",
        source_id: "SECOND-b",
      }),
    )
    expect(api.post).toHaveBeenCalledTimes(1)
  })
})
