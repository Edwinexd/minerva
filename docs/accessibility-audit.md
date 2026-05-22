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

### H5 - Errors not announced · WCAG 3.3.1 (A) ☑
The config save error was a plain `<p>` (no role). **Fix:** `role="alert"` on
the config error (the survey form already announces its form-level validation
message via `role="alert"`, which identifies the error in text per 3.3.1).
Per-field `aria-invalid`/`aria-describedby` is not added because these forms use
a single form-level error message rather than a per-field error model; that
would be a validation refactor and is noted as a future enhancement.

---

## 🟡 Medium

### M1 - Required indicated by colour/attribute only · WCAG 1.4.1 / 3.3.2 ☑
`message-feedback.tsx` red "required" text only; `home-page.tsx` name input
used only the `required` attribute with no visible cue.
**Fix:** visible "(required)" text / asterisk with accessible name.

### M2 - Focus-visible relies on ring-only with `outline-none` · WCAG 2.4.7 ☐
A handful of custom buttons (`chat-page.tsx`, `aegis-feedback-panel.tsx`) use
the same `focus-visible:ring-ring/50` indicator as the shared Button primitive,
i.e. the whole app's focus style. A visible ring is a valid focus indicator;
the real question is whether the `--ring` token at 50 % opacity clears the 3:1
non-text contrast threshold in both themes. **Deliberately not special-cased**
to 3 buttons (that would diverge from the rest of the UI); folded into the
colour-contrast verification item below - fix the token once, globally, if it
falls short.

### M3 - Admin nav not in a `<nav>` landmark · WCAG 1.3.1 ☑
`admin-layout.tsx` tab/select navigation lacked a landmark.
**Fix:** wrap in `<nav aria-label>`.

### M4 - Auto-dismiss copy toasts 1.5-2 s · WCAG 4.1.3 / 2.2.1 ☑
Copy confirmations in 5 files were short and unannounced.
**Fix:** announce via a visually-hidden `<output>` (implicit `role="status"`);
the message is non-essential and re-triggerable, so the auto-revert timing is
acceptable once announced.

---

## ℹ️ Not verified here (recommend a manual pass)

- **Colour contrast (1.4.3 / 1.4.11):** the CSS token palette in `index.css`
  was not measured against 4.5:1 (text) / 3:1 (UI) in both themes. Check
  `text-muted-foreground` and the focus ring colour especially.
- **200 % text zoom / 320 px reflow (1.4.4 / 1.4.10)** with real content.
- **Screen-reader walkthroughs** (NVDA + VoiceOver) of the chat, teacher, and
  admin flows - the bulk of the app is behind auth and never hit by the
  3-URL pa11y job. Recommend expanding pa11y coverage or adding authenticated
  axe component tests for those pages.

---

## Verified good (no action)

Skip link → focus-managed `<main>`; semantic landmarks; tables wrapped in
`overflow-x-auto` (1.4.10 reflow); labelled icon buttons (theme toggle,
language switcher, chat controls, feedback thumbs with `aria-pressed`);
`@base-ui/react` dialogs (focus trap / Escape / ARIA); favicon `alt=""`;
jsx-a11y strict + axe passing.
