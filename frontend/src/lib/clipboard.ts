/**
 * Copy text to the clipboard. Tries the modern `navigator.clipboard` API
 * first and falls back to the legacy `document.execCommand("copy")` via a
 * hidden textarea on insecure-context pages.
 *
 * Browsers only expose `navigator.clipboard.writeText` in "secure contexts"
 * (HTTPS pages, or http://localhost / http://127.0.0.1). Plain HTTP on a
 * LAN IP (http://192.168.x.y) is NOT a secure context, so the modern API
 * is `undefined` there and every Copy button silently does nothing. The
 * legacy fallback still works in that case.
 *
 * Returns `true` if a copy happened, `false` if both paths failed (e.g.
 * the document has no focus, or the user denied clipboard permission).
 */
export async function copyToClipboard(text: string): Promise<boolean> {
  if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text)
      return true
    } catch {
      // Fall through to legacy path; the clipboard API can also reject
      // when the document isn't focused or permissions denied.
    }
  }
  return legacyCopy(text)
}

function legacyCopy(text: string): boolean {
  if (typeof document === "undefined") return false
  const ta = document.createElement("textarea")
  ta.value = text
  // Off-screen but selectable. `position: fixed` keeps the page from
  // scrolling; `aria-hidden` keeps screen readers from announcing it.
  ta.setAttribute("readonly", "")
  ta.setAttribute("aria-hidden", "true")
  ta.style.position = "fixed"
  ta.style.top = "0"
  ta.style.left = "0"
  ta.style.width = "1px"
  ta.style.height = "1px"
  ta.style.padding = "0"
  ta.style.border = "0"
  ta.style.opacity = "0"
  document.body.appendChild(ta)
  try {
    ta.focus()
    ta.select()
    ta.setSelectionRange(0, ta.value.length)
    return document.execCommand("copy")
  } catch {
    return false
  } finally {
    document.body.removeChild(ta)
  }
}
