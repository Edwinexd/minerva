/**
 * Every path on Minerva's host written into the frontend or the backend
 * leads somewhere: an SPA page, a file in `public/`, or a backend endpoint
 * (see `brokenReason`). An interpolated value (`${id}`, `{token}`,
 * `$workspaceId`) may fill any one segment, so
 * `/workspaces/${id}/conversations/new` is checked as a path.
 *
 * The tests that render pages (`routes.test.tsx`, `pages.a11y.test.tsx`)
 * check the links those pages actually draw, values included; this scan
 * covers code no test renders and the URLs the backend hands out
 * (redirects, invite links, LTI launches).
 *
 * A path-like string that is not a link (a file path, an external API's
 * path) goes in the allowlist below with the reason.
 */
import fs from "node:fs"
import path from "node:path"
import ts from "typescript"
import { describe, expect, it } from "vitest"

import { HOLE, REPO, brokenReason } from "./links"

interface Found {
  where: string
  literal: string
  path: string
}

function walk(dir: string, keep: (p: string) => boolean, skipDirs: string[], acc: string[] = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name)
    if (e.isDirectory()) {
      if (!skipDirs.includes(e.name)) walk(p, keep, skipDirs, acc)
    } else if (keep(p)) acc.push(p)
  }
  return acc
}

// ── Frontend ────────────────────────────────────────────────────────────

/** String methods whose argument is a fragment to look for, not a link. */
const STRING_OPS = new Set([
  "startsWith", "endsWith", "includes", "split", "indexOf", "lastIndexOf",
  "replace", "replaceAll", "match", "test",
])
/** Calls whose path argument the callee prefixes with `/api` (and every `api.*`). */
const API_CALLS = new Set(["embedGet", "embedPost"])
/** JSX props a component hands to `api.*` (`ModelCatalogCard`). */
const API_PATH_PROPS = new Set(["enabledPath", "defaultPath", "benchmarkPath"])

function calleeName(call: ts.CallExpression): string {
  const e = call.expression
  if (ts.isIdentifier(e)) return e.text
  if (ts.isPropertyAccessExpression(e)) {
    return ts.isIdentifier(e.expression) ? `${e.expression.text}.${e.name.text}` : `.${e.name.text}`
  }
  return ""
}

function frontendPaths(): Found[] {
  const src = path.join(REPO, "frontend/src")
  const files = walk(
    src,
    (p) => /\.tsx?$/.test(p) && !/\.test\.tsx?$|\.d\.ts$|routeTree\.gen\.ts$/.test(p),
    ["test"],
  )
  const found: Found[] = []
  for (const file of files) {
    const sf = ts.createSourceFile(file, fs.readFileSync(file, "utf8"), ts.ScriptTarget.Latest, true)
    const visit = (n: ts.Node) => {
      let text: string | null = null
      if (ts.isStringLiteral(n) || ts.isNoSubstitutionTemplateLiteral(n)) text = n.text
      else if (ts.isTemplateExpression(n)) {
        // A query string or hash spliced in (`${location.searchStr}`) is
        // not part of the path; anything else stands for an id.
        text =
          n.head.text +
          n.templateSpans
            .map((s) => (/(search|searchStr|hash)$/.test(s.expression.getText()) ? "" : HOLE) + s.literal.text)
            .join("")
      }
      if (text?.startsWith("/") && !text.startsWith("//")) {
        // Climb out of `cond ? "/a" : "/b"`, `x ?? "/a"` and parentheses to
        // the expression the value ends up in.
        let arg: ts.Node = n
        while (
          ts.isParenthesizedExpression(arg.parent) ||
          ts.isAsExpression(arg.parent) ||
          (ts.isConditionalExpression(arg.parent) && arg.parent.condition !== arg) ||
          (ts.isBinaryExpression(arg.parent) &&
            [ts.SyntaxKind.QuestionQuestionToken, ts.SyntaxKind.BarBarToken].includes(arg.parent.operatorToken.kind))
        ) {
          arg = arg.parent
        }
        const p = arg.parent
        const call = ts.isCallExpression(p) && p.arguments[0] === arg ? calleeName(p) : ""
        const prop = ts.isJsxAttribute(p) ? p.name.getText() : ""
        const skip =
          ts.isImportDeclaration(p) ||
          call === "createFileRoute" ||
          ts.isExportDeclaration(p) ||
          (ts.isCallExpression(p) && STRING_OPS.has(call.split(".").pop()!)) ||
          API_CALLS.has(call) ||
          call.startsWith("api.") ||
          API_PATH_PROPS.has(prop)
        if (!skip) {
          const line = sf.getLineAndCharacterOfPosition(n.getStart()).line + 1
          found.push({
            where: `${path.relative(REPO, file)}:${line}`,
            literal: n.getText(),
            // Typed route paths (`/workspaces/$workspaceId`) get the sample too.
            path: text.replace(/\/\$[A-Za-z_]*(?=\/|$)/g, `/${HOLE}`),
          })
        }
      }
      ts.forEachChild(n, visit)
    }
    visit(sf)
  }
  return found
}

// ── Backend ─────────────────────────────────────────────────────────────

interface RustString {
  value: string
  start: number
  end: number
}

/**
 * Every string literal in a Rust file outside comments, plus the source
 * with literals and comments blanked (for brace matching).
 */
function lexRust(src: string): { strings: RustString[]; masked: string } {
  const strings: RustString[] = []
  const masked = src.split("")
  const blank = (from: number, to: number) => {
    for (let k = from; k < to; k++) if (masked[k] !== "\n") masked[k] = " "
  }
  const ident = (k: number) => /[A-Za-z0-9_]/.test(src[k] ?? "")
  let i = 0
  while (i < src.length) {
    const c = src[i]
    const d = src[i + 1]
    if (c === "/" && d === "/") {
      const e = src.indexOf("\n", i)
      const end = e < 0 ? src.length : e
      blank(i, end)
      i = end
    } else if (c === "/" && d === "*") {
      let depth = 1
      let j = i + 2
      while (j < src.length && depth) {
        if (src[j] === "/" && src[j + 1] === "*") {
          depth++
          j += 2
        } else if (src[j] === "*" && src[j + 1] === "/") {
          depth--
          j += 2
        } else j++
      }
      blank(i, j)
      i = j
    } else if (c === "'") {
      // A char literal; a lifetime has no closing quote.
      const m = /^'(?:\\(?:u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)|[^\\'])'/u.exec(src.slice(i, i + 16))
      if (m) blank(i, i + m[0].length)
      i += m ? m[0].length : 1
    } else {
      const raw = ident(i - 1) ? null : /^[bc]?r(#*)"/.exec(src.slice(i, i + 40))
      if (raw) {
        const close = `"${raw[1]}`
        const from = i + raw[0].length
        const to = src.indexOf(close, from)
        strings.push({ value: src.slice(from, to), start: i, end: to + close.length })
        blank(i, to + close.length)
        i = to + close.length
      } else if (c === '"' || (/[bc]/.test(c) && d === '"' && !ident(i - 1))) {
        let j = c === '"' ? i + 1 : i + 2
        let value = ""
        while (j < src.length && src[j] !== '"') {
          if (src[j] === "\\") {
            const e = src[j + 1]
            j += 2
            if (e === "\n") while (/\s/.test(src[j])) j++
            else value += e === "n" ? "\n" : e === "t" ? "\t" : e
          } else value += src[j++]
        }
        strings.push({ value, start: i, end: j + 1 })
        blank(i, j + 1)
        i = j + 1
      } else i++
    }
  }
  return { strings, masked: masked.join("") }
}

/** Spans of items under `#[cfg(test)]`: test code builds no real links. */
function testSpans(masked: string): Array<[number, number]> {
  const spans: Array<[number, number]> = []
  for (const m of masked.matchAll(/#\[cfg\(test\)\]/g)) {
    const from = m.index!
    const brace = masked.indexOf("{", from)
    const semi = masked.indexOf(";", from)
    if (semi >= 0 && (brace < 0 || semi < brace)) {
      spans.push([from, semi])
      continue
    }
    let depth = 0
    let j = brace
    for (; j < masked.length; j++) {
      if (masked[j] === "{") depth++
      else if (masked[j] === "}" && --depth === 0) break
    }
    spans.push([from, j])
  }
  return spans
}

/** Calls whose argument is a route definition, a JSON pointer or a fragment. */
const RUST_NOT_LINKS =
  /\.(route|nest|nest_service|route_service|pointer|pointer_mut|starts_with|ends_with|strip_prefix|strip_suffix|trim_start_matches|trim_end_matches|contains|split|find|replace)\(\s*$/

function backendPaths(): Found[] {
  const crates = path.join(REPO, "backend/crates")
  const files = walk(
    crates,
    (p) => p.endsWith(".rs") && path.basename(p) !== "build.rs",
    ["target", "tests", "examples", "benches"],
  )
  const found: Found[] = []
  for (const file of files) {
    const src = fs.readFileSync(file, "utf8")
    const { strings, masked } = lexRust(src)
    const spans = testSpans(masked)
    for (const s of strings) {
      if (spans.some(([a, b]) => s.start >= a && s.start <= b)) continue
      if (RUST_NOT_LINKS.test(src.slice(Math.max(0, s.start - 40), s.start))) continue
      let value = s.value
      // `format!("{}/lti/launch", base_url)`: a link once the base URL is
      // stripped. Any other leading placeholder is a directory.
      const lead = /^\{([A-Za-z_]*)\}(?=\/)/.exec(value)
      if (lead) {
        const arg = lead[1] || (/^\s*,\s*([^,)]+)/.exec(src.slice(s.end, s.end + 120))?.[1] ?? "")
        if (!/base/i.test(arg)) continue
        value = value.slice(lead[0].length)
      }
      if (!value.startsWith("/") || value.startsWith("//")) continue
      const line = src.slice(0, s.start).split("\n").length
      found.push({
        where: `${path.relative(REPO, file)}:${line}`,
        literal: JSON.stringify(s.value),
        path: value.replace(/\{[^{}]*\}/g, HOLE),
      })
    }
  }
  return found
}

// ── Not links ───────────────────────────────────────────────────────────

/** Literal as written (Rust: the string value, JSON-quoted) to why it is not a link. */
const NOT_LINKS: Record<string, string> = {
  '"/proc/self/status"': "file read by memprobe",
  '"/sys/fs/cgroup/memory.max"': "cgroup v2 file",
  '"/sys/fs/cgroup/memory/memory.limit_in_bytes"': "cgroup v1 file",
  '"/etc/minerva/slurm/known_hosts"': "mounted secret file",
  '"/etc/minerva/slurm/id_ed25519"': "mounted secret file",
  '"/en"': "language segment of an su.se URL in the dev seed",
  '"/vart-su/styrning/regelboken/"': "path on medarbetare.su.se",
  '"{}/chat/completions"': "LLM provider endpoint",
  '"{}/v1/messages"': "LLM provider endpoint",
  '"{base}/responses"': "LLM provider endpoint",
  '"{base}/models"': "LLM provider endpoint",
  '"{base}/v1/models"': "LLM provider endpoint",
  '"{base}/{route}/{token}"': "visual extraction worker URL; route is an /api/service path",
  '"/global-knowledge.resolved.json"': "build output file in OUT_DIR",
  '"{BASE}/mod/lti/auth.php"': "endpoint of the dev seed's fake Moodle",
  '"{BASE}/mod/lti/token.php"': "endpoint of the dev seed's fake Moodle",
  '"{BASE}/mod/lti/certs.php"': "endpoint of the dev seed's fake Moodle",
  '"{BASE}/mod/lti/services.php/CourseSection/{context_id}/bindings/1/memberships"':
    "endpoint of the dev seed's fake Moodle",
}

describe("paths on Minerva's host lead somewhere", () => {
  const all = [...frontendPaths(), ...backendPaths()]

  it("finds the links it is meant to check", () => {
    const literals = all.map((f) => f.literal)
    // One from each side, so a scanner that silently finds nothing fails.
    expect(literals).toContain('"/oauth/requests/{id}"')
    expect(literals).toContain('"/embed/courses/{}?token={}&lti_client_id={}"')
    expect(literals).toContain('"/data-handling"')
    expect(all.length).toBeGreaterThan(100)
  })

  it("every one resolves", () => {
    const broken = all.flatMap((f) => {
      if (f.literal in NOT_LINKS) return []
      const reason = brokenReason(f.path, { template: true })
      return reason ? [`${f.where}: ${f.literal} links to a non-existent page (${reason})`] : []
    })
    expect(broken, `Broken links:\n  ${broken.join("\n  ")}\n`).toEqual([])
  })

  it("every allowlist entry is still in use", () => {
    const literals = new Set(all.map((f) => f.literal))
    expect(Object.keys(NOT_LINKS).filter((l) => !literals.has(l))).toEqual([])
  })
})
