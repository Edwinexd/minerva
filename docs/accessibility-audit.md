# Accessibility Audit - Minerva frontend

**Date:** 2026-05-22
**Scope:** `frontend/` React SPA
**Standard:** DIGG webbriktlinjer → *Lagen om tillgänglighet till digital
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

Status legend: ☐ open · ☑ fixed in this branch.

---

## 🔴 Critical

### C1 - No accessibility statement (tillgänglighetsredogörelse) · DOS-lagen ☑
DOS-lagen requires every public-sector site to publish a statement with: the
compliance level, known shortcomings, the date of assessment, a **feedback /
reporting function** ("anmäl bristande tillgänglighet"), and a link to DIGG as
the enforcement body. None existed.
**Fix:** `/accessibility` route + `AccessibilityPage` component, en/sv content,
footer link, and added to the pa11y URL set.

### C2 - `prefers-reduced-motion` not implemented · WCAG 2.3.3 (AAA) / 2.2.2 ☑
No `@media (prefers-reduced-motion: reduce)` in `src/index.css`. Animations
with no fallback: `animate-bounce` (`chat-transcript.tsx`), `animate-pulse`
(`thinking-block.tsx`, `chat-page.tsx`, `ui/skeleton.tsx`), dialog zoom/fade
(`ui/alert-dialog.tsx`).
**Fix:** global reduced-motion block neutralising animations/transitions.

### C3 - Page titles missing on most routes · WCAG 2.4.2 (A) ☑
`use-document-title.ts` existed but was called on only a handful of pages.
Missing across all admin pages and 12 teacher course sub-pages.
**Fix:** `useDocumentTitle` added to every page component.

### C4 - Knowledge graph has no text alternative · WCAG 1.1.1 (A) ☑
`ForceGraph2D` canvas in `knowledge-graph-page.tsx` exposed nothing to AT.
**Fix:** `role="img"` + `aria-label` summary on the canvas wrapper and an
accessible relationship summary; the existing edge list is the data-table
alternative.

---

## 🟠 High

### H1 - Unlabelled form controls · WCAG 1.3.1 / 3.3.2 / 4.1.2 (A) ☑
- `documents-page.tsx` - `<input type="file">` (PDF + MBZ) no label
- `members-page.tsx` - eppn input (placeholder only) + native role `<select>`
- `external-invites-page.tsx` - read-only invite-URL input
- `admin/users-page.tsx` - owner-limit numeric input
- `root-layout.tsx` - dev user-switcher `<select>`
- several `<SelectTrigger>` without an associated name
**Fix:** `aria-label` / associated `<Label htmlFor>` on each.

### H2 - Status messages not announced · WCAG 4.1.3 (AA) ☑
Save success/error were plain `<span>`/`<p>`; loading states and RAG results
were not in live regions; chat "thinking" phase was silent.
**Fix:** `role="status"` / `role="alert"` + `aria-live`; `aria-busy` on the
streaming response while thinking.

### H3 - Home page has no `<h1>` · WCAG 1.3.1 / 2.4.6 (A/AA) ☑
`home-page.tsx` top heading was `<h2>`. Orphaned `<h4>`s under `<h2>` in
`admin/study-page.tsx`.
**Fix:** promote to `<h1>`; correct level nesting.

### H4 - `<html lang>` wrong on first paint · WCAG 3.1.1 (A) ☑
`index.html` hard-coded `lang="en"`; the i18n listener only corrects it after
React mounts, so Swedish users got `lang="en"` until hydration.
**Fix:** inline pre-paint script reading `minerva-language` localStorage,
mirroring the existing theme script.

### H5 - Errors not announced / not linked to fields · WCAG 3.3.1 (A) ☑
The config save error was a plain `<p>` (no role): now `role="alert"`. The
survey form previously showed a single form-level message; now
`validateAndSubmit` records the offending question id and the message renders
inline beneath that field, with `aria-invalid` + `aria-describedby` on the
Likert radiogroup / free-text control pointing at it (per-field error model).

---

## 🟡 Medium

### M1 - Required indicated by colour/attribute only · WCAG 1.4.1 / 3.3.2 ☑
`message-feedback.tsx` red "required" text only; `home-page.tsx` name input
used only the `required` attribute with no visible cue.
**Fix:** visible "(required)" text / asterisk with accessible name.

### M2 - Focus indicator contrast · WCAG 2.4.7 / 1.4.11 ☑
Measured: the light `--ring` was 2.59:1 on white (below 3:1) even at full
opacity, and the `ring-ring/50` halo can never reach 3:1 (a 50 % blend over
white floors at ~1.9:1). **Fix:** darkened light `--ring`/`--sidebar-ring`
0.708 -> 0.62 (now 3.64:1); the primitives' compliant indicator is the
full-opacity `border-ring`, the `/50` ring stays as a decorative halo; the
global default outline and the 5 custom ring-only buttons now use full-opacity
`outline-ring` / `ring-ring`. See contrast results below.

### M3 - Admin nav not in a `<nav>` landmark · WCAG 1.3.1 ☑
`admin-layout.tsx` tab/select navigation lacked a landmark.
**Fix:** wrap in `<nav aria-label>`.

### M4 - Auto-dismiss copy toasts 1.5-2 s · WCAG 4.1.3 / 2.2.1 ☑
Copy confirmations in 5 files were short and unannounced.
**Fix:** announce via a visually-hidden `<output>` (implicit `role="status"`);
the message is non-essential and re-triggerable, so the auto-revert timing is
acceptable once announced.

---

## Colour contrast (1.4.3 / 1.4.11) - measured ☑

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

## Reflow & zoom (1.4.4 / 1.4.10) - verified ☑

Viewport is `width=device-width, initial-scale=1.0` (no `maximum-scale` /
`user-scalable=no`, so pinch/text zoom to 200 % is allowed). No fixed-width
container exceeds 320 px (only `w-[220px]`, `min-w-[14rem]`, `min-w-[12rem]`,
all in `flex-wrap` rows), and data tables sit in `overflow-x-auto`. No change
needed.

## Authenticated-page coverage ☑

`src/test/pages.a11y.test.tsx` renders the real authenticated pages that the
public pa11y job can't reach and runs axe (WCAG 2a/2aa/21a/21aa/22aa tags) on
each loaded state: admin user management, teacher config / documents / members,
and the student new-chat surface. It stubs the router (`Link` -> `<a>`) and
seeds a `QueryClient` with fixtures so each page renders its real content (each
test also asserts a known string is present, so the axe check can't pass on an
empty skeleton). Runs in the same vitest job as the primitive tests.

## ℹ️ Still recommend a manual pass

- **Screen-reader walkthroughs** (NVDA + VoiceOver) of the chat, teacher, and
  admin flows - automated axe catches programmatic violations, but only a human
  with a screen reader can judge announcement quality and flow.

---

## Verified good (no action)

Skip link → focus-managed `<main>`; semantic landmarks; tables wrapped in
`overflow-x-auto` (1.4.10 reflow); labelled icon buttons (theme toggle,
language switcher, chat controls, feedback thumbs with `aria-pressed`);
`@base-ui/react` dialogs (focus trap / Escape / ARIA); favicon `alt=""`;
jsx-a11y strict + axe passing.
