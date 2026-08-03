import { useEffect } from "react"

const APP_NAME = "Minerva"

/**
 * Sets `document.title` to `${title} · Minerva` while the calling component
 * is mounted, and restores the previous title on unmount. Pass `null` or an
 * empty string to render just the bare app name.
 *
 * Pass `undefined` to opt out entirely and leave `document.title` alone.
 * That is for layouts that normally title the page but have a nested route
 * with a more specific title: React runs child effects before parent ones,
 * so a layout that always writes would overwrite the child's value.
 *
 * This is the WCAG 2.4.2 (Page Titled) hook; call it from every top-level
 * route component so screen-reader users, tab strips, and browser history
 * can distinguish pages.
 */
export function useDocumentTitle(title: string | null | undefined): void {
  useEffect(() => {
    if (title === undefined) return
    const previous = document.title
    document.title = title ? `${title} · ${APP_NAME}` : APP_NAME
    return () => {
      document.title = previous
    }
  }, [title])
}
