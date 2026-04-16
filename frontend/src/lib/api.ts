const API_BASE = "/api"

export interface ApiErrorBody {
  code: string
  params?: Record<string, string>
  message?: string
}

/// Thrown from api helpers for non-2xx backend responses. Preserves the
/// backend's stable `code` + `params` so the UI can translate via i18next
/// (see `useApiErrorMessage`). The `message` field holds the backend's
/// English fallback for logs / devtools; don't render it directly.
export class ApiError extends Error {
  readonly code: string
  readonly params: Record<string, string>
  readonly status: number

  constructor(status: number, body: ApiErrorBody) {
    super(body.message || body.code || `HTTP ${status}`)
    this.name = "ApiError"
    this.status = status
    this.code = body.code || "internal"
    this.params = body.params ?? {}
  }
}

function devHeaders(): Record<string, string> {
  const devUser = localStorage.getItem("minerva-dev-user")
  if (devUser) {
    return { "X-Dev-User": devUser }
  }
  return {}
}

/// If the Shibboleth session has expired, Apache mod_shib redirects API
/// requests to the IdP. With `redirect: "manual"` fetch returns an
/// `opaqueredirect` response we can detect. A full page navigation
/// re-triggers mod_shib in the top frame so the browser can complete the
/// IdP handshake and return the user to where they were.
///
/// Returns true if session expiry was handled (caller should stop).
function handleSessionExpired(res: Response): boolean {
  if (res.type === "opaqueredirect" || res.status === 401) {
    window.location.reload()
    return true
  }
  return false
}

async function parseErrorBody(res: Response): Promise<ApiErrorBody> {
  try {
    const body = (await res.json()) as Partial<ApiErrorBody>
    if (body && typeof body.code === "string") {
      return {
        code: body.code,
        params: body.params ?? {},
        message: body.message,
      }
    }
  } catch {
    // fall through
  }
  return { code: "internal", message: res.statusText }
}

async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, {
    ...options,
    redirect: "manual",
    headers: {
      "Content-Type": "application/json",
      ...devHeaders(),
      ...options?.headers,
    },
  })

  if (handleSessionExpired(res)) {
    return new Promise<T>(() => {})
  }

  if (!res.ok) {
    throw new ApiError(res.status, await parseErrorBody(res))
  }

  return res.json()
}

async function uploadFile<T>(path: string, file: File): Promise<T> {
  const formData = new FormData()
  formData.append("file", file)

  const res = await fetch(`${API_BASE}${path}`, {
    method: "POST",
    redirect: "manual",
    headers: devHeaders(),
    body: formData,
  })

  if (handleSessionExpired(res)) {
    return new Promise<T>(() => {})
  }

  if (!res.ok) {
    throw new ApiError(res.status, await parseErrorBody(res))
  }

  return res.json()
}

export const api = {
  get: <T>(path: string) => request<T>(path),
  post: <T>(path: string, body: unknown) =>
    request<T>(path, { method: "POST", body: JSON.stringify(body) }),
  put: <T>(path: string, body: unknown) =>
    request<T>(path, { method: "PUT", body: JSON.stringify(body) }),
  patch: <T>(path: string, body: unknown) =>
    request<T>(path, { method: "PATCH", body: JSON.stringify(body) }),
  delete: <T>(path: string) => request<T>(path, { method: "DELETE" }),
  upload: <T>(path: string, file: File) => uploadFile<T>(path, file),
}
