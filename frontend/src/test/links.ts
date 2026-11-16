/**
 * Whether a path on Minerva's own host leads somewhere: a page in the SPA's
 * route tree, a file in `public/`, or a path the backend serves itself.
 * Shared by the static link scan (`links.test.ts`) and the tests that render
 * pages and check the links they draw.
 */
import fs from "node:fs"
import path from "node:path"
import { createMemoryHistory, createRouter } from "@tanstack/react-router"
import type { AnyRoute } from "@tanstack/react-router"
import { QueryClient } from "@tanstack/react-query"

import { routeTree } from "@/routeTree.gen"

export const REPO = path.resolve(__dirname, "../../..")
const PUBLIC_DIR = path.join(REPO, "frontend/public")

/**
 * Stands in for an interpolated value in a path written in the source
 * (`${id}`, `{token}`, `$workspaceId`). It may fill any one segment, static
 * or not: `${tab}` can be `config`.
 */
export const HOLE = "\u0000"

export function makeRouter(url = "/") {
  return createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: [url] }),
    context: { queryClient: new QueryClient() },
  })
}

let shared: ReturnType<typeof makeRouter> | null = null
const router = () => (shared ??= makeRouter())

/** Static path segments anywhere in the tree (`courses`, `conversations`, ...). */
let statics: Set<string> | null = null
function staticSegments(): Set<string> {
  if (statics) return statics
  statics = new Set()
  for (const route of Object.values(router().routesById) as AnyRoute[]) {
    for (const seg of route.fullPath.split("/")) if (seg && !seg.startsWith("$")) statics.add(seg)
  }
  return statics
}

/**
 * The SPA route `pathname` lands on, or why it lands nowhere. A param equal
 * to a static segment (`/workspaces/courses/...`) means the path was put
 * together from the wrong pieces, so it counts as broken too.
 */
export function resolveSpa(pathname: string): { routeId: string; params: Record<string, string> } | { error: string } {
  const matches = router().matchRoutes(pathname, {})
  const leaf = matches[matches.length - 1]
  // Matching is fuzzy: an unknown path returns its deepest known parent
  // (`/admin/lti/nope` gives `/admin/lti`), so the leaf has to account for
  // the whole path.
  const params = (leaf?.params ?? {}) as Record<string, string>
  const route = leaf && (router().routesById[leaf.routeId as keyof ReturnType<typeof router>["routesById"]] as AnyRoute)
  const rebuilt = route?.fullPath
    .split("/")
    .map((seg: string) => (seg === "$" ? (params._splat ?? "") : seg.startsWith("$") ? params[seg.slice(1)] : seg))
    .join("/")
  const trim = (p: string) => p.replace(/\/+$/, "")
  if (!leaf || leaf.routeId === "__root__" || trim(rebuilt ?? "") !== trim(pathname)) {
    return { error: "matches no page" }
  }
  for (const [name, value] of Object.entries(params)) {
    if (name !== "_splat" && staticSegments().has(value)) {
      return { error: `its ${name} is "${value}", a path segment, not an id` }
    }
  }
  return { routeId: leaf.routeId, params }
}

/**
 * The backend's own public `/lti/*` endpoints, read from `public_router` in
 * `routes/lti.rs` (everything else under `/lti` is an SPA page).
 */
let ltiRoutes: RegExp[] | null = null
function backendLtiRoutes(): RegExp[] {
  if (ltiRoutes) return ltiRoutes
  const src = fs.readFileSync(path.join(REPO, "backend/crates/minerva-server/src/routes/lti.rs"), "utf8")
  const start = src.indexOf("pub fn public_router")
  const body = src.slice(start, src.indexOf("\n}\n", start))
  ltiRoutes = [...body.matchAll(/\.route\(\s*"([^"]+)"/g)].map(
    ([, r]) => new RegExp(`^/lti${r.replace(/\{[^}]+\}/g, "[^/]+")}$`),
  )
  if (ltiRoutes.length === 0) throw new Error("found no routes in lti::public_router")
  return ltiRoutes
}

/** Leaf routes (pages, not layouts) split into segments. */
let leaves: string[][] | null = null
function leafRoutes(): string[][] {
  if (leaves) return leaves
  leaves = (Object.values(router().routesById) as AnyRoute[])
    .filter((r) => r.id !== "__root__" && !(r.children && Object.keys(r.children).length))
    .map((r) => r.fullPath.replace(/\/+$/, "").split("/").slice(1))
  return leaves
}

/**
 * `resolveSpa` for a path with holes in it: some page has to fit it, holes
 * filling any one segment (or, at the end, none).
 */
function resolveTemplate(pathname: string): string | null {
  const segs = pathname.replace(/\/+$/, "").split("/").slice(1)
  const shapes = [segs]
  if (segs.length && segs[segs.length - 1] === HOLE) shapes.push(segs.slice(0, -1))
  let misuse: string | null = null
  for (const shape of shapes) {
    for (const route of leafRoutes()) {
      const splat = route[route.length - 1] === "$"
      const fixed = splat ? route.slice(0, -1) : route
      if (splat ? shape.length < fixed.length : shape.length !== fixed.length) continue
      let ok = true
      let wrong: string | null = null
      fixed.forEach((r, k) => {
        const seg = shape[k]
        const re = new RegExp(`^${seg.split(HOLE).map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join(".*")}$`)
        if (r.startsWith("$")) {
          if (!seg.includes(HOLE) && staticSegments().has(seg)) wrong ??= `"${seg}" sits where ${r} goes`
        } else if (!re.test(r)) ok = false
      })
      if (!ok) continue
      if (!wrong) return null
      misuse ??= wrong
    }
  }
  return misuse ?? "matches no page"
}

/**
 * Why `href` leads nowhere, or null when it resolves. Only paths on
 * Minerva's host are judged; `mailto:`, fragments and other hosts pass.
 *
 * Paths under `/api`, `/mcp` and `/.well-known` are the backend's own and
 * are not checked here: they are requests, not pages.
 */
export function brokenReason(href: string, { template = false } = {}): string | null {
  if (!href.startsWith("/") || href.startsWith("//")) return null
  const pathname = href.split(/[?#]/)[0]
  if (pathname === "/") return null
  if (/^\/(api|mcp|\.well-known)(\/|$)/.test(pathname)) return null
  if (pathname.startsWith("/lti/") && backendLtiRoutes().some((r) => r.test(pathname))) return null
  const asset = path.join(PUBLIC_DIR, pathname)
  if (asset.startsWith(PUBLIC_DIR + path.sep) && fs.existsSync(asset) && fs.statSync(asset).isFile()) {
    return null
  }
  if (template) return resolveTemplate(pathname)
  const result = resolveSpa(pathname)
  return "error" in result ? result.error : null
}

/**
 * Throws, naming each one, if any link under `root` leads to a page that
 * does not exist.
 */
export function expectLinksResolve(root: ParentNode = document) {
  const anchors = Array.from(root.querySelectorAll<HTMLAnchorElement>("a[href]"))
  const broken = anchors.flatMap((a) => {
    const href = a.getAttribute("href")!
    const reason = brokenReason(href)
    return reason ? [`"${a.textContent?.trim()}" links to a non-existent page: ${href} (${reason})`] : []
  })
  if (broken.length) throw new Error(`Broken links:\n  ${broken.join("\n  ")}`)
  return anchors
}
