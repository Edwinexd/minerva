# Accessibility Audit - Minerva frontend

Two passes so far. The 2026-08-03 re-audit is at the bottom; the
2026-05-22 baseline below is kept as-is for reference.

**Date:** 2026-05-22
**Scope:** `frontend/` React SPA
**Standard:** DIGG webbriktlinjer -> *Lagen om tillgänglighet till digital
offentlig service* (DOS-lagen), which mandates **EN 301 549**, i.e.
**WCAG 2.1 AA**. DIGG is migrating its supervision toward **WCAG 2.2 AA**, so
2.2-only criteria are included. Stockholm University / DSV is a public-sector
body, so the law applies in full.

## Method

- Static lint: `eslint` with `jsx-a11y` **strict** preset (passes clean).
- Component axe tests (`vitest-axe`) and `pa11y-ci` (htmlcs + axe) in CI.
- Manual source review across all 124 `src` files, grouped by WCAG criterion.

## Summary

The codebase is unusually well prepared: a working skip link, semantic
`header`/`nav`/`main`/`footer` landmarks, a focus-managed `<main>`, mostly
labelled icon buttons, dialogs built on `@base-ui/react` (focus trap + Escape
handled natively), and a11y linting wired into pre-commit and CI. ESLint
passes clean.

The gaps below are the runtime/behavioural issues that static analysis and the
3-URL pa11y job cannot catch - plus **one legal gap** (no accessibility
statement).

Status legend: [ ] open · [x] fixed in this branch.

---

## Critical

### C1 - No accessibility statement (tillgänglighetsredogörelse) · DOS-lagen [x]
DOS-lagen requires every public-sector site to publish a statement with: the
compliance level, known shortcomings, the date of assessment, a **feedback /
reporting function** ("anmäl bristande tillgänglighet"), and a link to DIGG as
the enforcement body. None existed.
**Fix:** `/accessibility` route + `AccessibilityPage` component, en/sv content,
footer link, and added to the pa11y URL set.

### C2 - `prefers-reduced-motion` not implemented · WCAG 2.3.3 (AAA) / 2.2.2 [x]
No `@media (prefers-reduced-motion: reduce)` in `src/index.css`. Animations
with no fallback: `animate-bounce` (`chat-transcript.tsx`), `animate-pulse`
(`thinking-block.tsx`, `chat-page.tsx`, `ui/skeleton.tsx`), dialog zoom/fade
(`ui/alert-dialog.tsx`).
**Fix:** global reduced-motion block neutralising animations/transitions.

### C3 - Page titles missing on most routes · WCAG 2.4.2 (A) [x]
`use-document-title.ts` existed but was called on only a handful of pages.
Missing across all admin pages and 12 teacher course sub-pages.
**Fix:** `useDocumentTitle` added to every page component.

### C4 - Knowledge graph has no text alternative · WCAG 1.1.1 (A) [x]
`ForceGraph2D` canvas in `knowledge-graph-page.tsx` exposed nothing to AT.
**Fix:** `role="img"` + `aria-label` summary on the canvas wrapper and an
accessible relationship summary; the existing edge list is the data-table
alternative.

---

## High

### H1 - Unlabelled form controls · WCAG 1.3.1 / 3.3.2 / 4.1.2 (A) [x]
- `documents-page.tsx` - `<input type="file">` (PDF + MBZ) no label
- `members-page.tsx` - eppn input (placeholder only) + native role `<select>`
- `external-invites-page.tsx` - read-only invite-URL input
- `admin/users-page.tsx` - owner-limit numeric input
- `root-layout.tsx` - dev user-switcher `<select>`
- several `<SelectTrigger>` without an associated name
**Fix:** `aria-label` / associated `<Label htmlFor>` on each.

### H2 - Status messages not announced · WCAG 4.1.3 (AA) [x]
Save success/error were plain `<span>`/`<p>`; loading states and RAG results
were not in live regions; chat "thinking" phase was silent.
**Fix:** `role="status"` / `role="alert"` + `aria-live`; `aria-busy` on the
streaming response while thinking.

### H3 - Home page has no `<h1>` · WCAG 1.3.1 / 2.4.6 (A/AA) [x]
`home-page.tsx` top heading was `<h2>`.
**Fix:** promote to `<h1>`; correct level nesting.

### H4 - `<html lang>` wrong on first paint · WCAG 3.1.1 (A) [x]
`index.html` hard-coded `lang="en"`; the i18n listener only corrects it after
React mounts, so Swedish users got `lang="en"` until hydration.
**Fix:** inline pre-paint script reading `minerva-language` localStorage,
mirroring the existing theme script.

### H5 - Errors not announced / not linked to fields · WCAG 3.3.1 (A) [x]
The config save error was a plain `<p>` (no role): now `role="alert"`. The
survey form previously showed a single form-level message; now
`validateAndSubmit` records the offending question id and the message renders
inline beneath that field, with `aria-invalid` + `aria-describedby` on the
Likert radiogroup / free-text control pointing at it (per-field error model).

---

## Medium

### M1 - Required indicated by colour/attribute only · WCAG 1.4.1 / 3.3.2 [x]
`message-feedback.tsx` red "required" text only; `home-page.tsx` name input
used only the `required` attribute with no visible cue.
**Fix:** visible "(required)" text / asterisk with accessible name.

### M2 - Focus indicator contrast · WCAG 2.4.7 / 1.4.11 [x]
Measured: the light `--ring` was 2.59:1 on white (below 3:1) even at full
opacity, and the `ring-ring/50` halo can never reach 3:1 (a 50 % blend over
white floors at ~1.9:1). **Fix:** darkened light `--ring`/`--sidebar-ring`
0.708 -> 0.62 (now 3.64:1); the primitives' compliant indicator is the
full-opacity `border-ring`, the `/50` ring stays as a decorative halo; the
global default outline and the 5 custom ring-only buttons now use full-opacity
`outline-ring` / `ring-ring`. See contrast results below.

### M3 - Admin nav not in a `<nav>` landmark · WCAG 1.3.1 [x]
`admin-layout.tsx` tab/select navigation lacked a landmark.
**Fix:** wrap in `<nav aria-label>`.

### M4 - Auto-dismiss copy toasts 1.5-2 s · WCAG 4.1.3 / 2.2.1 [x]
Copy confirmations in 5 files were short and unannounced.
**Fix:** announce via a visually-hidden `<output>` (implicit `role="status"`);
the message is non-essential and re-triggerable, so the auto-revert timing is
acceptable once announced.

---

## Colour contrast (1.4.3 / 1.4.11) - measured [x]

Every `--*-foreground` token was converted OKLCH -> linear sRGB and checked
against its background in both themes (script approach; pa11y's axe + htmlcs
runners also pass on the public pages). Failures found and fixed (all
light-theme; dark passed throughout):

| Pair | Before | After | Threshold |
|---|---|---|---|
| `muted-foreground` on `muted` | 4.34:1 | 4.64:1 | 4.5 (text) |
| `--ring` (focus) on background | 2.59:1 | 3.64:1 | 3.0 (UI) |
| `--input` border on background | 1.26:1 | 3.11:1 | 3.0 (UI) |

Fix = darken light `--muted-foreground` (0.556 -> 0.54), `--ring` (0.708 ->
0.62), `--input` (0.922 -> 0.66). `--border` is left as-is: it styles decorative
dividers / card edges, which 1.4.11 exempts.

## Reflow & zoom (1.4.4 / 1.4.10) - verified [x]

Viewport is `width=device-width, initial-scale=1.0` (no `maximum-scale` /
`user-scalable=no`, so pinch/text zoom to 200 % is allowed). No fixed-width
container exceeds 320 px (only `w-[220px]`, `min-w-[14rem]`, `min-w-[12rem]`,
all in `flex-wrap` rows), and data tables sit in `overflow-x-auto`. No change
needed.

## Authenticated-page coverage [x]

`src/test/pages.a11y.test.tsx` renders the real authenticated pages that the
public pa11y job can't reach and runs axe (WCAG 2a/2aa/21a/21aa/22aa tags) on
each loaded state: admin user management, teacher config / documents / members,
and the student new-chat surface. It stubs the router (`Link` -> `<a>`) and
seeds a `QueryClient` with fixtures so each page renders its real content (each
test also asserts a known string is present, so the axe check can't pass on an
empty skeleton). Runs in the same vitest job as the primitive tests.

## Still recommend a manual pass

- **Screen-reader walkthroughs** (NVDA + VoiceOver) of the chat, teacher, and
  admin flows - automated axe catches programmatic violations, but only a human
  with a screen reader can judge announcement quality and flow.

---

## Verified good (no action)

Skip link -> focus-managed `<main>`; semantic landmarks; tables wrapped in
`overflow-x-auto` (1.4.10 reflow); labelled icon buttons (theme toggle,
language switcher, chat controls, feedback thumbs with `aria-pressed`);
`@base-ui/react` dialogs (focus trap / Escape / ARIA); favicon `alt=""`;
jsx-a11y strict + axe passing.

---
---

# Re-audit - 2026-08-03

**Scope:** the 44 commits that touched `frontend/` since the audit above
(`6c235810..HEAD`), 124 -> 139 `src` files. New surfaces: admin courses
(bulk edit / archive / merge), Daisy imports, system Defaults, dev tools,
LTI platforms / approve / dynreg scope, model catalogs, the extracted
`ChatSurface`, and the teacher guide.
**Standard:** unchanged (WCAG 2.2 AA).

## Method

Same three layers, all green before the manual pass started:

| Layer | Result |
|---|---|
| `eslint` (jsx-a11y strict) | clean |
| `tsc -b`, `tsc -p tsconfig.test.json` | clean |
| `vitest` + axe (WCAG 2.2 AA tags) | 17/17 pass |
| `pa11y-ci` (htmlcs + axe, built preview) | 4/4 URLs, 0 errors |

Then a manual review of the changed surfaces, plus a numeric contrast
re-check (OKLCH -> linear sRGB) of every raw Tailwind palette class the
new code introduced.

## Summary

Nothing regressed in what the first audit fixed. What the new code did
was reintroduce the same *classes* of defect on surfaces the first audit
never saw: async status messages that are not announced (H2 last time),
and controls named only by their placeholder (H1 last time). One genuinely
new class showed up: 119 raw Tailwind palette colours across 16 files that
bypass the audited design tokens, three of which fail 1.4.3.

Status legend: [ ] open · [x] fixed on branch `a11y-audit-2026-08`.

---

## High

### R1 - Status messages not announced · WCAG 4.1.3 (AA) [x]
Every admin surface added since the last audit renders mutation results
and errors as plain `<p>`/`<span>`, so nothing reaches a screen reader:

- `admin/courses-page.tsx` bulk error + result summary, and 7 further
  mutation/query error sites (feature flags, migrate, merge, archive
  toggle, bulk edit)
- `admin/defaults-page.tsx` "Saved" flash and the save/reset/load errors
  (16 knobs)
- `admin/daisy-imports-page.tsx` auto-apply error, apply error, apply
  result summary
- `admin/chat-models-card.tsx`, `admin/model-catalog-card.tsx` per-row
  action errors and load failures
- `teacher/canvas-page.tsx` sync result and sync error
- `join/join-page.tsx` join failure

**Fix:** repo idiom applied throughout: `<output>` (implicit
`role="status"`) for success/result, `<p role="alert">` for errors.
`<output>` needs `block`; it is inline by default.

### R2 - Bulk archive/restore drops focus and destroys its own confirmation · WCAG 2.4.3 (A) + 4.1.3 (AA) [x]
`BulkActionBar` cleared the selection on full success, which unmounts the
bar via the `selectedCourses.length > 0` gate. The result summary lived
inside the bar and died in the same commit, and the confirm dialog was
destroyed rather than closed, so its focus restore never ran and focus
fell to `<body>`. Full success, the common case, produced no confirmation
at all. `AlertDialog`'s `finalFocus` cannot fix this for the same reason
the restore fails: nothing survives to run it.

**Fix:** the result is panel state now and renders outside the bar; the
summary is an `<output tabIndex={-1}>` that claims focus on mount. Pinned
by `src/test/bulk-actions.test.tsx`.

### R3 - Chat composer has no accessible name · WCAG 4.1.2 / 3.3.2 (A) [x]
`chat/chat-surface.tsx` composer was placeholder-only, so its name
disappears as soon as the student types. Affects the course chat and the
LTI embed, the two highest-traffic surfaces in the app. This predates the
first audit (it was `chat-page.tsx` then) and was missed, rather than
being a regression; it is the same defect H1 fixed for `members-page`.

**Fix:** `inputLabel` added to `ChatSurfaceLabels`, wired from both
routes, rendered as `aria-label` alongside the placeholder.

### R4 - Colour contrast below 4.5:1 in light theme · WCAG 1.4.3 (AA) [x]
Measured OKLCH -> linear sRGB, same method as the first audit:

| Class | Before | After | Site |
|---|---|---|---|
| `text-emerald-600` | 3.67:1 | 5.37:1 (`emerald-700`) | `defaults-page` saved flash |
| `text-amber-600` | 3.19:1 | 5.05:1 (`amber-700`) | `integration-keys-page`, `lti-platforms-page` |

The emerald flash also had no `dark:` variant. Root cause is wider: 119
raw palette `text-*` classes across 16 files now sit outside the token
set the first audit measured. The other 15 pairs checked pass in both
themes, so this was 3 sites plus a standing risk. Anything new that
reaches for a raw palette colour on `bg-background`/`bg-card` needs the
same check; the `-600` step is the one that lands just under threshold.

---

## Medium

### R5 - Two tables with no scroll container · WCAG 1.4.10 (AA) [x]
`admin/lti-platforms-page.tsx` bindings and NRPS tables carried
`font-mono` LTI context ids with no `overflow-x-auto` wrapper, so at 320
px the page itself scrolls horizontally. Every other table in the app
already wraps. **Fix:** wrapper on both, plus `break-all` on the context
id cells.

### R6 - Teacher course tab nav has no landmark · WCAG 1.3.1 (A) [x]
M3 fixed this for `admin-layout.tsx` only; `teacher/course-edit-page.tsx`
kept its bare mobile select + `Tabs`. **Fix:** `<nav aria-label>` around
both, matching the admin layout.

### R7 - No `<h1>` on authenticated pages · WCAG 1.3.1 / 2.4.6 (A/AA) [x]
Only 6 files in `src` had one. The teacher course page opened at `<h2>`;
the chat, embed and LTI routes had no heading element at all (`CardTitle`
renders a `<div>`).

**Fix:** `course-edit-page` heading promoted to `<h1>`; a visually-hidden
`<h1>` (course name) in `ChatSurface`, which covers chat and embed and
sits above the greeting's `<h2>`; `CardTitle` gained an `as` prop
(defaults to `div`) so the single-card LTI bind / setup / approve pages
can mark their title as the page heading.

### R8 - Aegis panel has no focus management · WCAG 2.4.3 (A) [x]
Below `aegisDrawerBreakpoint` the panel is a fixed drawer over the chat
with a dismiss backdrop, but focus never moved into it, never returned to
the "bring it back" pill, and Escape did nothing. **Fix:** the `<aside>`
is a named landmark and takes focus when the pill opens it; closing
returns focus to the pill; Escape closes it while focus is inside.

Deliberately *not* a focus trap: the same element is an in-flow rail at
and above the breakpoint, where trapping would be wrong. Both moves are
armed by an explicit click, never by the value: `panelVisible` is
storage-backed and defaults to true, so keying off the value alone would
grab focus on every chat page load. Pinned by
`src/test/aegis-panel.test.tsx`, including the page-load case.

---

## Low

### R9 - Admin layout overwrote a sub-route's page title · WCAG 2.4.2 (A) [x]
`/admin/lti-approve/:id` sets a specific title, but React runs child
effects before parent ones, so `admin-layout.tsx` overwrote it with the
generic "Admin - LTI". **Fix:** `useDocumentTitle(undefined)` now means
"leave the title alone", and the layout passes it for the deep sub-flows
it does not own.

---

## Test coverage added

`src/test/pages.a11y.test.tsx` covered 6 pages; roughly 10 admin surfaces
added since had no rendered-axe coverage, and pa11y still only reaches the
4 public URLs (the rest are behind Shibboleth). Added:

- axe passes for admin course management (loaded, and with rows selected
  so the bulk bar is in the tree), Daisy imports, system Defaults (one
  knob per widget kind), and role rules.
- `src/test/bulk-actions.test.tsx`: drives select -> Archive -> Confirm
  and asserts the summary outlives the bar and holds focus (R2).
- `src/test/aegis-panel.test.tsx`: no focus theft on first render, focus
  round-trip on close/reopen, Escape-to-close (R8).

26 tests across 4 files, all passing.

## Still open

- **Screen-reader walkthroughs** (NVDA + VoiceOver) of the chat, teacher
  and admin flows. Carried over from the first audit and still the one
  thing none of these layers substitutes for.
- **Raw palette drift.** 119 `text-<palette>-<step>` classes across 16
  files sit outside the audited tokens. Nothing enforces a contrast check
  on new ones; the three that failed were found by measuring, not by a
  test.
