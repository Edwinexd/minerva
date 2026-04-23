import { useEffect } from "react"

const APP_NAME = "Minerva"

/**
 * Sets `document.title` to `${title} · Minerva` while the calling component
 * is mounted, and restores the previous title on unmount. Pass `null` or an
 * empty string to render just the bare app name.
 *
 * This is the WCAG 2.4.2 (Page Titled) hook; call it from every top-level
 * route component so screen-reader users, tab strips, and browser history
 * can distinguish pages.
 */
export function useDocumentTitle(title: string | null | undefined): void {
  useEffect(() => {
    const previous = document.title
    document.title = title ? `${title} · ${APP_NAME}` : APP_NAME
    return () => {
      document.title = previous
    }
  }, [title])
}
