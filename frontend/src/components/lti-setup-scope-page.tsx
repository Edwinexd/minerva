import { useEffect, useMemo, useState } from "react"
import { useTranslation } from "react-i18next"
import { useMutation } from "@tanstack/react-query"
import { Route as SetupRoute } from "@/routes/lti/setup.$platformId"
import { useApiErrorMessage } from "@/lib/use-api-error"
import { useDocumentTitle } from "@/lib/use-document-title"
import {
  DYNREG_CHANNEL,
  type DynregBroadcastMessage,
} from "@/lib/lti-dynreg-channel"
import { Button, buttonVariants } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"

/**
 * Trust-scope picker for LTI 1.3 Dynamic Registration. The Rust dynreg
 * handler at /lti/dynamic-register completes the IMS server-to-server
 * handshake and 303s here so the LMS admin can pick which eppn domains
 * to *suggest*; final activation still requires a Minerva integrator to
 * click Approve on the platform.
 *
 * This page is public (RootLayout skips its userQuery on /lti/setup, same
 * way it does for /lti/bind). It posts the chosen suggestion to the
 * public-but-platform-scoped endpoint `/lti/dynamic-register/<id>/scope`,
 * then renders a success state with the IMS-spec close postMessage so the
 * LMS popup closes cleanly.
 */
export function LtiSetupScopePage() {
  const { t } = useTranslation("admin")
  const { t: tCommon } = useTranslation("common")
  useDocumentTitle(tCommon("pageTitles.ltiSetup"))
  const formatError = useApiErrorMessage()

  const { platformId } = SetupRoute.useParams()
  const { name: platformName, issuer } = SetupRoute.useSearch()

  // Walk the issuer hostname up by one subdomain at a time, keeping only
  // suffixes with at least one dot, dropping bare IPs and `localhost`.
  // Mirrors the server-side suggester so admins see consistent options;
  // pure UI helper here.
  const suggestions = useMemo(() => {
    if (!issuer) return [] as string[]
    const u = (() => {
      try {
        return new URL(issuer)
      } catch {
        return null
      }
    })()
    const host = u?.hostname ?? issuer
    if (!host || host.includes(":")) return []
    const labels = host.split(".")
    if (labels.every((l) => l.length > 0 && /^\d+$/.test(l))) return []
    const out: string[] = []
    for (let i = 0; i < labels.length; i++) {
      const candidate = labels.slice(i).join(".")
      if (candidate.includes(".")) out.push(candidate)
    }
    return out
  }, [issuer])

  // Always offer the host itself first (covers `localhost`, bare IPs,
  // and TLDs the walker filtered out).
  const hostItself = useMemo(() => {
    if (!issuer) return null
    try {
      return new URL(issuer).hostname
    } catch {
      return null
    }
  }, [issuer])

  const presets = useMemo(() => {
    const all = [...(hostItself ? [hostItself] : []), ...suggestions]
    return Array.from(new Set(all))
  }, [hostItself, suggestions])

  // Multi-select: any subset of preset suggestions PLUS arbitrary
  // additional entries via the free-text field. Default-check only the
  // most specific preset (admins can broaden) so the "narrow trust"
  // default is sticky for the careless click-through.
  const [checked, setChecked] = useState<Set<string>>(
    () => new Set(presets.length > 0 ? [presets[0]] : []),
  )
  const [customInput, setCustomInput] = useState("")
  const [submitted, setSubmitted] = useState(false)

  const togglePreset = (preset: string) => {
    setChecked((prev) => {
      const next = new Set(prev)
      if (next.has(preset)) next.delete(preset)
      else next.add(preset)
      return next
    })
  }

  // Scope endpoint lives at /lti/*, NOT /api/*, so api module's `/api`
  // prefix doesn't apply. Direct fetch; no auth needed (the platform_id
  // is the addressable key, the suggestion is advisory only).
  const mutation = useMutation({
    mutationFn: async (domains: string) => {
      const res = await fetch(
        `/lti/dynamic-register/${encodeURIComponent(platformId)}/scope`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ domains }),
        },
      )
      if (!res.ok) {
        const body = await res.text().catch(() => "")
        throw new Error(`HTTP ${res.status}: ${body || res.statusText}`)
      }
      return res.json() as Promise<{ recorded: boolean; domains: string[] }>
    },
    onSuccess: () => {
      setSubmitted(true)
      // Intentionally do NOT post `org.imsglobal.lti.close` here. The LMS
      // closes its dialog the instant that message arrives, which would
      // mean the user never sees the success state with the "Open
      // Minerva to approve" / "Close" buttons. The close happens only
      // when the user clicks Close (or the "Open Minerva to approve"
      // flow finishes and they come back here). Spec-compliant: the
      // close message is "the tool tells the LMS we're done"; nothing
      // says it has to fire instantly.
    },
  })

  const submit = (e: React.FormEvent) => {
    e.preventDefault()
    // Server side does its own normalisation + validation; we just
    // assemble the union as a comma-separated string.
    const fromChecked = Array.from(checked)
    const fromCustom = customInput
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0)
    const all = Array.from(new Set([...fromChecked, ...fromCustom]))
    mutation.mutate(all.join(", "))
  }

  const closePopup = () => {
    try {
      ;(window.opener || window.parent)?.postMessage(
        { subject: "org.imsglobal.lti.close" },
        "*",
      )
    } catch {
      /* ignore */
    }
    window.close()
  }

  // While the success card is showing, listen for the approve flow in
  // another tab. If THIS platform gets approved, fire the LTI close
  // postMessage so Moodle dismisses its dialog automatically. Mounted
  // only after submission so we don't react to stale messages while the
  // user is still filling in the form.
  useEffect(() => {
    if (!submitted) return
    if (typeof BroadcastChannel === "undefined") return
    const ch = new BroadcastChannel(DYNREG_CHANNEL)
    const onMessage = (e: MessageEvent<DynregBroadcastMessage>) => {
      if (e.data?.type === "approved" && e.data.platformId === platformId) {
        closePopup()
      }
    }
    ch.addEventListener("message", onMessage)
    return () => {
      ch.removeEventListener("message", onMessage)
      ch.close()
    }
  }, [submitted, platformId])

  if (submitted) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>{t("ltiSetupScope.doneTitle")}</CardTitle>
          <CardDescription>{t("ltiSetupScope.doneBody")}</CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <p className="text-sm text-muted-foreground">
            {t("ltiSetupScope.donePrompt")}
          </p>
          <div className="flex flex-wrap gap-2">
            <a
              href={`/admin/lti-approve/${encodeURIComponent(platformId)}`}
              target="_blank"
              rel="noopener noreferrer"
              className={buttonVariants()}
            >
              {t("ltiSetupScope.openMinervaButton")}
            </a>
            <Button variant="outline" onClick={closePopup}>
              {t("ltiSetupScope.closeButton")}
            </Button>
          </div>
        </CardContent>
      </Card>
    )
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>
          {platformName
            ? t("ltiSetupScope.titleWithName", { name: platformName })
            : t("ltiSetupScope.titleFallback")}
        </CardTitle>
        <CardDescription>{t("ltiSetupScope.lede")}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="rounded-md border border-amber-500/60 bg-amber-50/60 px-3 py-2 text-sm text-amber-900 dark:bg-amber-950/30 dark:text-amber-200">
          <strong>{t("ltiSetupScope.warnHeading")}</strong>{" "}
          {t("ltiSetupScope.warnBody")}
        </div>

        <form onSubmit={submit} className="space-y-3">
          <fieldset className="space-y-2">
            <legend className="text-sm font-medium">
              {t("ltiSetupScope.legend")}
            </legend>
            {presets.length === 0 ? (
              <p className="text-xs text-muted-foreground">
                {t("ltiSetupScope.noPresets")}
              </p>
            ) : (
              presets.map((preset) => (
                <label
                  key={preset}
                  className="flex cursor-pointer items-center gap-2 text-sm"
                >
                  <input
                    type="checkbox"
                    name="scope"
                    value={preset}
                    checked={checked.has(preset)}
                    onChange={() => togglePreset(preset)}
                  />
                  <code className="rounded bg-muted px-1.5 py-0.5">
                    {preset}
                  </code>
                </label>
              ))
            )}
          </fieldset>

          <div className="space-y-1">
            <Label htmlFor="lti-setup-custom" className="text-sm font-medium">
              {t("ltiSetupScope.customLabel")}
            </Label>
            <Input
              id="lti-setup-custom"
              value={customInput}
              onChange={(e) => setCustomInput(e.target.value)}
              placeholder="partner.org, other.example.edu"
              className="font-mono"
            />
            <p className="text-xs text-muted-foreground">
              {t("ltiSetupScope.customHint")}
            </p>
          </div>

          {checked.size === 0 && customInput.trim().length === 0 && (
            <p className="text-xs text-amber-700 dark:text-amber-400">
              {t("ltiSetupScope.emptyWarn")}
            </p>
          )}

          {mutation.isError && (
            <p className="text-sm text-destructive">
              {formatError(mutation.error)}
            </p>
          )}

          <div className="flex flex-wrap gap-2">
            <Button type="submit" disabled={mutation.isPending}>
              {t("ltiSetupScope.submit")}
            </Button>
            <Button
              type="button"
              variant="outline"
              onClick={closePopup}
              disabled={mutation.isPending}
            >
              {t("ltiSetupScope.skip")}
            </Button>
          </div>
        </form>
        <p className="text-xs text-muted-foreground">
          {t("ltiSetupScope.afterNote")}
        </p>
      </CardContent>
    </Card>
  )
}
