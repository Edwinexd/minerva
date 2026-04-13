const API_BASE = "/api"

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
    const body = await res.json().catch(() => ({ error: res.statusText }))
    throw new Error(body.error || res.statusText)
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
    const body = await res.json().catch(() => ({ error: res.statusText }))
    throw new Error(body.error || res.statusText)
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
