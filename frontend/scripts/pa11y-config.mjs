/**
 * Builds the pa11y-ci URL list, including the pages behind auth.
 *
 * The public routes in `.pa11yci.json` render a logged-out shell, so most of
 * the app (chat, teacher tabs, admin panels, the LTI embed) has never had
 * real-browser accessibility coverage. Those are exactly the pages where
 * layout-dependent rules bite: axe's `scrollable-region-focusable` and
 * colour-contrast only fire once an element genuinely overflows or paints,
 * which jsdom can never reproduce.
 *
 * Reaching them needs no Shibboleth. With `MINERVA_DEV_MODE=true` the backend
 * takes the caller's identity from an `X-Dev-User` header, so pa11y just sends
 * that. Course and conversation ids are generated fresh by each dev seed, so
 * this script seeds the database, reads the ids back, and writes a config with
 * them substituted in.
 *
 * Usage: node scripts/pa11y-config.mjs [--out .a11y/pa11yci.json]
 * Expects a backend serving the built SPA at $MINERVA_BASE_URL.
 */
import { createHmac } from "node:crypto"
import { dirname } from "node:path"
import { mkdir, readFile, writeFile } from "node:fs/promises"

const BASE = process.env.MINERVA_BASE_URL ?? "http://127.0.0.1:3000"
const ADMIN = process.env.MINERVA_A11Y_ADMIN ?? "devadmin@su.se"
const HMAC_SECRET = process.env.MINERVA_HMAC_SECRET ?? ""
const outFlag = process.argv.indexOf("--out")
// Guard the -1: `argv[indexOf(...) + 1]` with no flag is `argv[0]`, the node
// binary itself, which then gets happily overwritten.
const OUT = outFlag === -1 ? ".a11y/pa11yci.json" : process.argv[outFlag + 1]

const asAdmin = { "X-Dev-User": ADMIN }

async function api(path, init = {}) {
  const res = await fetch(`${BASE}/api${path}`, {
    ...init,
    headers: { "Content-Type": "application/json", ...asAdmin, ...init.headers },
  })
  if (!res.ok) {
    throw new Error(`${init.method ?? "GET"} /api${path} -> ${res.status} ${await res.text()}`)
  }
  return res.json()
}

async function waitForBackend() {
  for (let attempt = 0; attempt < 120; attempt++) {
    try {
      const res = await fetch(`${BASE}/api/health`)
      if (res.ok) return
    } catch {
      // Not listening yet.
    }
    await new Promise((r) => setTimeout(r, 1000))
  }
  throw new Error(`backend never became healthy at ${BASE}`)
}

/**
 * Mint an embed token the same way `create_embed_token` does:
 * base64url("<course>:<user>:<expiry>:<hex hmac-sha256 of the first three>").
 * Minting it here rather than calling the integration API keeps this script
 * free of API-key provisioning; the secret is already CI-controlled.
 */
function mintEmbedToken(courseId, userId) {
  if (!HMAC_SECRET) throw new Error("MINERVA_HMAC_SECRET must be set to mint an embed token")
  const expires = Math.floor(Date.now() / 1000) + 3600
  const payload = `${courseId}:${userId}:${expires}`
  const sig = createHmac("sha256", HMAC_SECRET).update(payload).digest("hex")
  return Buffer.from(`${payload}:${sig}`).toString("base64url")
}

await waitForBackend()

// Destructive reseed; this database exists only for this run.
const report = await api("/admin/dev/seed", { method: "POST" })
console.log(`seeded: ${report.users} users, ${report.courses} courses, ${report.documents} docs`)

const me = await api("/auth/me")
const courses = await api("/courses")
// The seeder's first course ("Intro Programming") is the plain baseline: admin
// owns it, students are enrolled, and it has conversations with real turns,
// which is what makes the transcript long enough to actually scroll.
const course = courses.find((c) => c.name.startsWith("Intro Programming")) ?? courses[0]
if (!course) throw new Error("dev seed produced no courses")

const conversations = await api(`/courses/${course.id}/conversations`)
const conversation = conversations[0]
if (!conversation) {
  throw new Error(`seeded course ${course.id} has no conversations to audit`)
}

const token = mintEmbedToken(course.id, me.id)

// Every page a teacher/admin can reach that renders seeded data. Anything
// whose markup is a pure duplicate of another entry is left out to keep the
// job's wall-clock sane; the goal is one instance of each distinct layout.
const authenticated = [
  "/",
  `/course/${course.id}`,
  `/course/${course.id}/new`,
  // Populated transcript: the only state in which the scroll region exists.
  `/course/${course.id}/${conversation.id}`,
  `/teacher/courses/${course.id}`,
  `/teacher/courses/${course.id}/members`,
  `/teacher/courses/${course.id}/documents`,
  `/teacher/courses/${course.id}/conversations`,
  `/teacher/courses/${course.id}/rag`,
  `/teacher/courses/${course.id}/usage`,
  `/teacher/courses/${course.id}/invite`,
  "/teacher",
  "/teacher-help",
  "/admin/courses",
  "/admin/users",
  "/admin/rules",
]

// `.pa11yci.json` holds paths, not absolute URLs, so $MINERVA_BASE_URL is the
// only place the host is decided. Keeping absolute URLs there meant a run on a
// non-default port silently audited nothing on the public routes.
const base = JSON.parse(await readFile(".pa11yci.json", "utf8"))
const config = {
  ...base,
  urls: [
    // Public routes stay anonymous: they are reachable logged-out in prod and
    // that shell is what an unauthenticated visitor actually gets.
    ...base.urls.map((path) => `${BASE}${path}`),
    ...authenticated.map((path) => ({
      url: `${BASE}${path}`,
      headers: asAdmin,
    })),
    // The LTI embed authenticates by token, not by dev header. Its viewport is
    // deliberately small: the iframe is where an overflowing transcript was
    // first caught, and the rule only applies when content actually overflows.
    {
      url: `${BASE}/embed/${course.id}?token=${token}`,
      viewport: { width: 640, height: 480 },
    },
  ],
}

await mkdir(dirname(OUT), { recursive: true })
await writeFile(OUT, `${JSON.stringify(config, null, 2)}\n`)
console.log(`wrote ${OUT} with ${config.urls.length} urls (${base.urls.length} public)`)
