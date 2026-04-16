import { useTranslation } from "react-i18next"
import { ApiError } from "./api"

/// Translate an error thrown from an `api.*` call via i18next. Falls back to
/// the backend's English `message` for non-ApiError throws (network glitches,
/// bugs) so the user always sees something useful.
export function useApiErrorMessage(): (err: unknown) => string {
  const { t } = useTranslation("errors")
  return (err: unknown) => {
    if (err instanceof ApiError) {
      return t(err.code, { ...err.params, defaultValue: err.message })
    }
    if (err instanceof Error) {
      return err.message
    }
    return t("internal")
  }
}

/// One-shot variant when you already have an error in scope. Use the hook
/// form for reactive components.
export function formatApiError(
  err: unknown,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  if (err instanceof ApiError) {
    return t(`errors:${err.code}`, { ...err.params, defaultValue: err.message })
  }
  if (err instanceof Error) {
    return err.message
  }
  return t("errors:internal")
}

/// A LocalizedMessage emitted by the backend in non-error payloads (canvas
/// warnings/errors arrays). Same shape as ApiError's body, different carrier.
export interface LocalizedMessage {
  code: string
  params?: Record<string, string>
}

export function useLocalizedMessage(): (msg: LocalizedMessage) => string {
  const { t } = useTranslation("errors")
  return (msg) => t(msg.code, { ...(msg.params ?? {}), defaultValue: msg.code })
}
