/**
 * Internal links and redirects must land on a real route with the ids they
 * started with. Two ways this broke before:
 *
 *   - The embed header read the course id by splitting the pathname on
 *     `/embed/`, so after the move to `/embed/courses/{id}` it linked to
 *     `/workspaces/courses/{id}/conversations/new`.
 *   - A redirect built from a string `href` is not type-checked, so a typo
 *     in it only shows up when someone follows an old link.
 *
 * These drive the real route tree, so a new legacy redirect is covered
 * without being listed here. `links.test.ts` checks the paths written in
 * the source; this checks what pages produce at runtime, ids included.
 */
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest"
import { render, waitFor, within } from "@testing-library/react"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { RouterProvider, createMemoryHistory, createRouter } from "@tanstack/react-router"
import type { AnyRoute } from "@tanstack/react-router"

import "@/i18n"

vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>()
  const pending = () => new Promise(() => {})
  return {
    ...actual,
    api: { get: vi.fn(pending), post: vi.fn(pending), put: vi.fn(pending), delete: vi.fn(pending) },
  }
})

import { routeTree } from "@/routeTree.gen"
import { userQuery } from "@/lib/queries"
import type { User } from "@/lib/types"
import { expectLinksResolve, resolveSpa } from "./links"

const COURSE = "b0a9fe4e-6312-4061-b60f-fc659a85bfe8"

function makeRouter(url: string) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  // Role-dependent redirects (`/admin`, `/teacher`) read the current user.
  queryClient.setQueryData(userQuery.queryKey, {
    id: "u1",
    eppn: "admin@su.se",
    display_name: "Admin",
    role: "admin",
  } as User)
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: [url] }),
    context: { queryClient },
  })
  return { router, queryClient }
}

function expectPage(href: string) {
  const result = resolveSpa(new URL(href, "http://localhost").pathname)
  if ("error" in result) throw new Error(`link to a non-existent page: ${href} (${result.error})`)
  return result
}

describe("redirects", () => {
  const { router: probe } = makeRouter("/")
  const routes = (Object.values(probe.routesById) as AnyRoute[]).filter((r) => r.options.beforeLoad)

  it("covers at least the known legacy addresses", () => {
    const paths = routes.map((r) => r.fullPath)
    expect(paths).toContain("/embed/$courseId")
    expect(paths).toContain("/course/$courseId/new")
  })

  it.each(routes.map((r) => [r.fullPath, r] as const))(
    "%s lands on a real route with its ids",
    async (_path, route) => {
      // One distinct id per param, so a dropped or swapped id is visible.
      const ids: string[] = []
      const path = route.fullPath
        .split("/")
        .map((seg: string, i: number) => {
          if (seg === "$") return "members"
          if (!seg.startsWith("$")) return seg
          const id = `00000000-0000-4000-8000-${String(i).padStart(12, "0")}`
          ids.push(id)
          return id
        })
        .join("/")

      const { router } = makeRouter(`${path}?token=t`)
      await router.load()
      const landed = router.state.location
      if (landed.pathname === path) return // a guard, not a redirect

      const result = expectPage(landed.href)
      const target = router.routesById[result.routeId as keyof typeof router.routesById] as AnyRoute
      expect(target.options.beforeLoad, `${path} redirects to another redirect`).toBeUndefined()
      for (const id of ids) expect(landed.pathname).toContain(id)
      if (route.fullPath === "/teacher/courses/$courseId/$") {
        expect(landed.pathname.endsWith("/members")).toBe(true)
      }
    },
  )
})

describe("embed header", () => {
  const fetchMock = vi.fn()
  beforeEach(() => {
    fetchMock.mockImplementation((input: RequestInfo | URL) => {
      const url = String(input)
      if (url.startsWith(`/api/embed/courses/${COURSE}/me`)) {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              id: "u1",
              eppn: "teacher@su.se",
              display_name: "Teacher",
              lti_client_id: null,
              course_role: "teacher",
            }),
            { headers: { "content-type": "application/json" } },
          ),
        )
      }
      return new Promise(() => {})
    })
    vi.stubGlobal("fetch", fetchMock)
  })
  afterEach(() => {
    vi.unstubAllGlobals()
    fetchMock.mockReset()
  })

  it.each([`/embed/courses/${COURSE}?token=t`, `/embed/${COURSE}?token=t`])(
    "%s links to real routes for the course",
    async (url) => {
      const { router, queryClient } = makeRouter(url)
      // RootLayout reads window.location to tell an embed from the app.
      window.history.replaceState(null, "", url)
      await router.load()
      window.history.replaceState(null, "", router.state.location.href)
      render(
        <QueryClientProvider client={queryClient}>
          <RouterProvider router={router} />
        </QueryClientProvider>,
      )

      const header = within(document.querySelector("header")!)
      await header.findByRole("link", { name: /Minerva/ })
      expectLinksResolve()
      await waitFor(() =>
        expect(fetchMock).toHaveBeenCalledWith(`/api/embed/courses/${COURSE}/me?token=t`),
      )
      await header.findByRole("link", { name: /teacher/i })

      // Every in-app link on the page goes somewhere real, and the header's
      // point at this course.
      for (const a of expectLinksResolve()) {
        if (!a.closest("header") || !a.getAttribute("href")!.startsWith("/")) continue
        const { params } = expectPage(a.getAttribute("href")!)
        expect(Object.values(params)).toContain(COURSE)
      }
      expect(header.getByRole("link", { name: /Minerva/ }).getAttribute("href")).toBe(
        `/workspaces/${COURSE}/conversations/new`,
      )
      expect(header.getByRole("link", { name: /teacher/i }).getAttribute("href")).toBe(
        `/teacher/workspaces/${COURSE}`,
      )
    },
  )
})
