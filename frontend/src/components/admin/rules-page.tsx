import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { Trans, useTranslation } from "react-i18next"
import { useId, useMemo, useState } from "react"
import {
  adminRoleRuleAttributeValuesQuery,
  adminRoleRulesQuery,
} from "@/lib/queries"
import { api } from "@/lib/api"
import { useApiErrorMessage } from "@/lib/use-api-error"
import {
  ROLE_RULE_ATTRIBUTES,
  ROLE_RULE_OPERATORS,
  ROLE_RULE_TARGET_ROLES,
  type RoleRule,
  type RoleRuleAttribute,
  type RoleRuleAttributeValueSuggestion,
  type RoleRuleOperator,
  type RoleRuleTargetRole,
} from "@/lib/types"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Skeleton } from "@/components/ui/skeleton"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"

export function RoleRulesPanel() {
  const { t } = useTranslation("admin")
  const { data: rules, isLoading } = useQuery(adminRoleRulesQuery)

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          <CardTitle>{t("rules.title")}</CardTitle>
          <CardDescription>
            {t("rules.description")}
            <br />
            <span className="text-xs">
              <Trans
                i18nKey="rules.negatedNote"
                ns="admin"
                components={[<code key="c1" />, <code key="c2" />]}
              />
            </span>
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <CreateRuleForm />
          {isLoading && (
            <div className="space-y-2">
              {Array.from({ length: 3 }).map((_, i) => (
                <Skeleton key={i} className="h-24 w-full" />
              ))}
            </div>
          )}
          {!isLoading && rules && rules.length === 0 && (
            <p className="text-sm text-muted-foreground">
              {t("rules.empty")}
            </p>
          )}
          {rules?.map((rule) => <RuleCard key={rule.id} rule={rule} />)}
        </CardContent>
      </Card>
    </div>
  )
}

function CreateRuleForm() {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const [name, setName] = useState("")
  const [targetRole, setTargetRole] = useState<RoleRuleTargetRole>("teacher")

  const mutation = useMutation({
    mutationFn: () =>
      api.post<RoleRule>("/admin/role-rules", {
        name: name.trim(),
        target_role: targetRole,
        enabled: true,
      }),
    onSuccess: () => {
      setName("")
      setTargetRole("teacher")
      queryClient.invalidateQueries({ queryKey: ["admin", "role-rules"] })
    },
  })

  return (
    <form
      className="flex flex-wrap items-end gap-2 rounded border p-3"
      onSubmit={(e) => {
        e.preventDefault()
        if (name.trim()) mutation.mutate()
      }}
    >
      <div className="space-y-1">
        <label htmlFor="rule-name" className="text-xs font-medium">{t("rules.form.ruleName")}</label>
        <input
          id="rule-name"
          className="block h-8 w-64 rounded border bg-background px-2 text-sm"
          placeholder={t("rules.form.ruleNamePlaceholder")}
          value={name}
          onChange={(e) => setName(e.target.value)}
          aria-invalid={mutation.isError || undefined}
          aria-describedby={mutation.isError ? "rule-name-error" : undefined}
        />
      </div>
      <div className="space-y-1">
        <label className="text-xs font-medium">{t("rules.form.targetRole")}</label>
        <Select value={targetRole} onValueChange={(v) => v && setTargetRole(v as RoleRuleTargetRole)}>
          <SelectTrigger className="h-8 w-36 text-sm" aria-label={t("rules.form.targetRole")}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {ROLE_RULE_TARGET_ROLES.map((role) => (
              <SelectItem key={role} value={role}>
                {t(`rules.form.${role}`)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <Button
        type="submit"
        size="sm"
        disabled={!name.trim() || mutation.isPending}
      >
        {t("rules.form.createRule")}
      </Button>
      {mutation.isError && (
        <span id="rule-name-error" role="alert" className="text-xs text-destructive">
          {formatError(mutation.error)}
        </span>
      )}
    </form>
  )
}

function RuleCard({ rule }: { rule: RoleRule }) {
  const { t } = useTranslation("admin")
  const queryClient = useQueryClient()
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["admin", "role-rules"] })

  const updateMutation = useMutation({
    mutationFn: (body: Pick<RoleRule, "name" | "target_role" | "enabled">) =>
      api.put(`/admin/role-rules/${rule.id}`, body),
    onSuccess: invalidate,
  })

  const deleteMutation = useMutation({
    mutationFn: () => api.delete(`/admin/role-rules/${rule.id}`),
    onSuccess: invalidate,
  })

  return (
    <Card className="border">
      <CardHeader className="flex flex-row items-start justify-between gap-2 space-y-0">
        <div className="space-y-1">
          <CardTitle className="flex items-center gap-2 text-base">
            {rule.name}
            <Badge variant={rule.enabled ? "default" : "secondary"}>
              {rule.enabled ? t("rules.card.enabled") : t("rules.card.disabled")}
            </Badge>
            <Badge variant="outline">→ {rule.target_role}</Badge>
          </CardTitle>
          <CardDescription>
            {rule.conditions.length === 0
              ? t("rules.card.noConditions")
              : t("rules.card.matchesWhen", { count: rule.conditions.length })}
          </CardDescription>
        </div>
        <div className="flex gap-2">
          <Button
            size="sm"
            variant="outline"
            onClick={() =>
              updateMutation.mutate({
                name: rule.name,
                target_role: rule.target_role,
                enabled: !rule.enabled,
              })
            }
            disabled={updateMutation.isPending}
          >
            {rule.enabled ? t("rules.card.disable") : t("rules.card.enable")}
          </Button>
          <Button
            size="sm"
            variant="destructive"
            onClick={() => {
              if (confirm(t("rules.card.confirmDelete", { name: rule.name }))) deleteMutation.mutate()
            }}
            disabled={deleteMutation.isPending}
          >
            {t("rules.card.delete")}
          </Button>
        </div>
      </CardHeader>
      <CardContent className="space-y-2">
        {rule.conditions.map((c) => (
          <ConditionRow key={c.id} condition={c} />
        ))}
        <AddConditionForm ruleId={rule.id} />
      </CardContent>
    </Card>
  )
}

function ConditionRow({
  condition,
}: {
  condition: RoleRule["conditions"][number]
}) {
  const { t } = useTranslation("admin")
  const queryClient = useQueryClient()

  const deleteMutation = useMutation({
    mutationFn: () => api.delete(`/admin/role-rules/conditions/${condition.id}`),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: ["admin", "role-rules"] }),
  })

  return (
    <div className="flex items-center gap-2 rounded border bg-muted/30 p-2 text-xs">
      <code className="rounded bg-background px-1.5 py-0.5">{condition.attribute}</code>
      <Badge variant="outline">{condition.operator}</Badge>
      <code className="flex-1 break-all rounded bg-background px-1.5 py-0.5 font-mono">
        {condition.value}
      </code>
      <Button
        size="sm"
        variant="ghost"
        className="h-6 text-xs"
        aria-label={t("rules.condition.removeLabel")}
        onClick={() => deleteMutation.mutate()}
        disabled={deleteMutation.isPending}
      >
        ×
      </Button>
    </div>
  )
}

function AddConditionForm({ ruleId }: { ruleId: string }) {
  const { t } = useTranslation("admin")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const [attribute, setAttribute] = useState<RoleRuleAttribute>("entitlement")
  const [operator, setOperator] = useState<RoleRuleOperator>("contains")
  const [value, setValue] = useState("")

  // Pulled once at panel mount via the cached query; switching attributes
  // is then instant. Soft-failure: if the suggestions endpoint errors we
  // simply render no datalist and the admin types freely.
  const { data: suggestionPayload } = useQuery(adminRoleRuleAttributeValuesQuery)

  // Stable id for the datalist <-> input list= pairing. Each rule's form
  // gets its own so two open forms don't fight over the same id.
  const datalistId = useId()

  // Suggestions for the currently-selected attribute, with regex / not_regex
  // values automatically escaped so the admin can drop a suggestion straight
  // into a regex condition without rewriting backslashes. The original
  // (un-escaped) value is still readable in the option label via i18n so
  // they can see what they're picking.
  const suggestions: RoleRuleAttributeValueSuggestion[] = useMemo(() => {
    const raw = suggestionPayload?.by_attribute[attribute] ?? []
    if (operator !== "regex" && operator !== "not_regex") return raw
    return raw.map((s) => ({ ...s, value: escapeForRegex(s.value) }))
  }, [suggestionPayload, attribute, operator])

  const mutation = useMutation({
    mutationFn: () =>
      api.post(`/admin/role-rules/${ruleId}/conditions`, {
        attribute,
        operator,
        value,
      }),
    onSuccess: () => {
      setValue("")
      queryClient.invalidateQueries({ queryKey: ["admin", "role-rules"] })
    },
  })

  return (
    <form
      className="flex flex-wrap items-end gap-2 rounded border border-dashed p-2 text-xs"
      onSubmit={(e) => {
        e.preventDefault()
        if (value.trim()) mutation.mutate()
      }}
    >
      <Select value={attribute} onValueChange={(v) => v && setAttribute(v as RoleRuleAttribute)}>
        <SelectTrigger className="h-7 w-32 text-xs" aria-label={t("rules.condition.attributeLabel")}>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {ROLE_RULE_ATTRIBUTES.map((a) => (
            <SelectItem key={a} value={a}>
              {a}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <Select value={operator} onValueChange={(v) => v && setOperator(v as RoleRuleOperator)}>
        <SelectTrigger className="h-7 w-32 text-xs" aria-label={t("rules.condition.operatorLabel")}>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {ROLE_RULE_OPERATORS.map((o) => (
            <SelectItem key={o} value={o}>
              {o}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <input
        className="h-7 flex-1 min-w-[14rem] rounded border bg-background px-2 font-mono text-xs"
        placeholder={t("rules.condition.valuePlaceholder")}
        aria-label={t("rules.condition.valuePlaceholder")}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        list={suggestions.length > 0 ? datalistId : undefined}
      />
      {suggestions.length > 0 && (
        // Native <datalist> autocomplete: the input stays free-text, so an
        // admin can still create a rule for a value we haven't observed yet
        // (the suggestions only cover values seen on >= min_users users).
        // The label shows the original value + observed user count; the
        // option value is what gets dropped into the input (regex-escaped
        // when the operator is regex/not_regex, raw otherwise).
        <datalist id={datalistId}>
          {suggestions.map((s) => (
            <option
              key={s.value}
              value={s.value}
              label={t("rules.condition.suggestionLabel", { count: s.user_count })}
            />
          ))}
        </datalist>
      )}
      <Button
        type="submit"
        size="sm"
        className="h-7 text-xs"
        disabled={!value.trim() || mutation.isPending}
      >
        {t("rules.condition.add")}
      </Button>
      {suggestionPayload && (
        <span className="w-full text-[10px] text-muted-foreground">
          {suggestions.length > 0
            ? t("rules.condition.suggestionHint", {
                count: suggestions.length,
                minUsers: suggestionPayload.min_users,
              })
            : t("rules.condition.suggestionEmpty", {
                minUsers: suggestionPayload.min_users,
              })}
        </span>
      )}
      {mutation.isError && (
        <span className="text-destructive">{formatError(mutation.error)}</span>
      )}
    </form>
  )
}

/**
 * Escape a literal string so it matches itself when used as a regex. Used
 * when offering observed values as suggestions for the regex / not_regex
 * operators: the admin is free to edit the escaped value into a broader
 * pattern, but the default insert is a safe literal match. Mirrors the set
 * of characters that have special meaning in Rust's `regex` crate (the
 * backend's matcher).
 */
function escapeForRegex(value: string): string {
  return value.replace(/[\\^$.*+?()[\]{}|/]/g, "\\$&")
}
