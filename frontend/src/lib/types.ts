import type { LocalizedMessage } from "./use-api-error"

export interface User {
  id: string
  eppn: string
  display_name: string | null
  role: "student" | "teacher" | "integrator" | "admin"
  suspended?: boolean
  privacy_acknowledged_at: string | null
}

export interface Course {
  id: string
  name: string
  description: string | null
  owner_id: string
  context_ratio: number
  temperature: number
  model: string
  system_prompt: string | null
  max_chunks: number
  min_score: number
  strategy: string
  /**
   * Orthogonal to `strategy`: when TRUE, the model gains access to a
   * tool catalog during a research/thinking phase before the final
   * writeup. Both `simple` and `flare` honour this flag. The API will
   * reject a save that picks tool use on a model that doesn't
   * support it (see backend `model_capabilities`).
   */
  tool_use_enabled: boolean
  embedding_provider: string
  embedding_model: string
  /**
   * Bumped each time `embedding_provider` or `embedding_model`
   * rotates. Surfaced for diagnostics; the config form doesn't
   * read it directly, but the documents tab uses it to correlate
   * post-rotation re-ingestion progress with the live generation.
   */
  embedding_version: number
  daily_token_limit: number
  active: boolean
  created_at: string
  updated_at: string
  /** Viewing user's course_member role, if any. Drives UI gating for TAs. */
  my_role: "student" | "ta" | "teacher" | null
  /**
   * Per-course feature flags. Resolved server-side through the same
   * path the backend uses, so the UI's "is X enabled" check matches
   * what the runtime actually does. Drives hide/show on KG-related
   * tabs, badges, and dialogs.
   */
  feature_flags: CourseFeatureFlags
  /**
   * `VT2026` / `HT2025` etc. Set by the Daisy auto-import phase;
   * drives the per-semester grouping on the My Courses page. Null
   * for ad-hoc / manually-created courses.
   */
  semester_label: string | null
  /**
   * Daisy linkage block when this course was auto-imported. Drives
   * the "Auto-managed by Daisy sync" badge + info / syllabus links
   * on the course settings page.
   */
  daisy: DaisyMeta | null
  /**
   * TRUE when this course was created by the Daisy auto-import
   * phase. Used by the frontend to show a status badge and (later)
   * gate UI affordances that shouldn't fight the sync.
   */
  auto_managed: boolean
  /**
   * Short course code (e.g. `PROG2`). Stable identifier across
   * `name` renames; populated by the Daisy auto-import. Rendered as
   * a chip on the My Courses tile when present.
   */
  course_code: string | null
}

export interface DaisyMeta {
  momenttillf_id: string
  info_url: string | null
  syllabus_url: string | null
  unit: string | null
  last_synced_at: string | null
}

export interface CourseFeatureFlags {
  /**
   * Course knowledge graph V1: per-doc kind classification + linker
   * + graph viewer + assignment-refusal addendum + adversarial
   * chunk filter. Off by default until an admin opts the course in.
   */
  course_kg: boolean
  /**
   * Aegis prompt-coaching feedback panel. When TRUE the chat page
   * renders a third right-side column with per-prompt scoring and
   * history. Resolves through the same path as `course_kg`
   * (course-scoped row > global > default false).
   */
  aegis: boolean
  /**
   * Concept knowledge graph (eureka-2): per-document concept and
   * relation extraction, persisted as an admin-viewable graph.
   * Distinct from `course_kg`, which is the document-level graph.
   * Off by default; admins flip per course to expose the concept
   * graph viewer and the extract/run-dedup actions.
   */
  concept_graph: boolean
  /**
   * Study mode: turns the course into a research-evaluation
   * pipeline (consent screen, pre-survey, N hardcoded tasks,
   * post-survey, thank-you + lockout). When TRUE the course
   * landing page redirects members to the study pipeline instead
   * of the regular conversation list, and forces Aegis on for the
   * duration. Configuration lives in `study_courses` /
   * `study_tasks` / `study_surveys`; this flag is the runtime gate.
   */
  study_mode: boolean
}

// ── Study mode ────────────────────────────────────────────────────

export type StudyStage = "consent" | "pre_survey" | "task" | "post_survey" | "done"

export interface StudyTaskView {
  task_index: number
  title: string
  description: string
  /**
   * Per-round Aegis gate. When FALSE, `TaskRunner` passes
   * `aegisEnabled={false}` to `ChatWindow`, hiding the panel,
   * banner and rewrite button and suppressing live analyzer
   * calls. Defaults TRUE on the server for back-compat; the
   * DM2731 preset sets rounds 1 and 3 to FALSE and round 2 to
   * TRUE.
   */
  aegis_enabled: boolean
}

export interface StudyState {
  stage: StudyStage
  current_task_index: number
  number_of_tasks: number
  completion_gate_kind: string
  consent_html: string
  thank_you_html: string
  consented_at: string | null
  pre_survey_completed_at: string | null
  post_survey_completed_at: string | null
  locked_out_at: string | null
  /** Populated only while `stage === "task"`. */
  current_task: StudyTaskView | null
  /** Populated only while `stage === "task"`. */
  current_task_conversation_id: string | null
}

export interface StudySurveyQuestion {
  id: string
  ord: number
  /**
   * `section_heading` is display-only (used to break long surveys
   * into named sections like "System Usability"); the form never
   * collects an answer for it. `likert` and `free_text` are
   * answer-bearing.
   */
  kind: "likert" | "free_text" | "section_heading"
  prompt: string
  likert_min: number | null
  likert_max: number | null
  likert_min_label: string | null
  likert_max_label: string | null
  /**
   * When false the participant may submit without answering this
   * question. Always false for `section_heading`.
   */
  is_required: boolean
  /**
   * Withdraw-on-answer kill switch (likert-only). When the
   * participant answers with this value, the server short-circuits
   * the pipeline to `done` regardless of stage. Used for GDPR-style
   * consent questions where "No" should withdraw the participant
   * cleanly. Null when no kill switch is configured.
   */
  kill_on_value: number | null
}

export interface StudySurveyAnswer {
  question_id: string
  likert_value: number | null
  free_text_value: string | null
}

export interface StudySurvey {
  kind: "pre" | "post"
  questions: StudySurveyQuestion[]
  /** Existing answers if the participant is resuming a half-filled survey. */
  existing: StudySurveyAnswer[]
}

export interface StudyStartTaskResponse {
  task_index: number
  conversation_id: string
}

export interface StudyFinishTaskResponse {
  stage: StudyStage
  current_task_index: number
  is_last_task: boolean
}

export interface AdminUser {
  id: string
  eppn: string
  display_name: string | null
  role: string
  suspended: boolean
  role_manually_set: boolean
  owner_daily_token_limit: number
  created_at: string
  updated_at: string
}

export type RoleRuleAttribute =
  | "eppn"
  | "displayName"
  | "affiliation"
  | "entitlement"
  | "mail"
  | "cn"
  | "sn"
  | "givenName"

export const ROLE_RULE_ATTRIBUTES: RoleRuleAttribute[] = [
  "eppn",
  "displayName",
  "affiliation",
  "entitlement",
  "mail",
  "cn",
  "sn",
  "givenName",
]

export type RoleRuleOperator =
  | "contains"
  | "not_contains"
  | "regex"
  | "not_regex"

export const ROLE_RULE_OPERATORS: RoleRuleOperator[] = [
  "contains",
  "not_contains",
  "regex",
  "not_regex",
]

export interface RoleRuleCondition {
  id: string
  rule_id: string
  attribute: string
  operator: string
  value: string
}

/**
 * Roles a role auto-promotion rule may grant. `admin` is excluded on
 * purpose: admins are always env-sourced via MINERVA_ADMINS so a stale rule
 * can't strand a permission. `integrator` (Teacher + site-wide integration
 * powers) is allowed; the backend mirrors this in `validate_target_role`.
 */
export type RoleRuleTargetRole = "student" | "teacher" | "integrator"

export const ROLE_RULE_TARGET_ROLES: RoleRuleTargetRole[] = [
  "student",
  "teacher",
  "integrator",
]

export interface RoleRule {
  id: string
  name: string
  target_role: RoleRuleTargetRole
  enabled: boolean
  created_at: string
  updated_at: string
  conditions: RoleRuleCondition[]
}

/**
 * One suggested value for a rule-condition attribute. Returned by
 * `/admin/role-rules/attribute-values` after filtering to values observed on
 * at least `min_users` distinct users. The admin can still type any value;
 * suggestions are an autocomplete convenience, not an allowlist.
 */
export interface RoleRuleAttributeValueSuggestion {
  value: string
  user_count: number
}

export interface RoleRuleAttributeValues {
  /** Per-attribute suggestion buckets; attributes without enough observations are omitted. */
  by_attribute: Partial<Record<RoleRuleAttribute, RoleRuleAttributeValueSuggestion[]>>
  /** Distinct-user threshold the backend filtered with; surface in the UI hint. */
  min_users: number
}

export interface UsageRecord {
  user_id: string
  course_id: string
  date: string
  prompt_tokens: number
  completion_tokens: number
  embedding_tokens: number
  /**
   * Research-phase prompt-token share of `prompt_tokens`. Lets the
   * teacher/admin usage views nest research / writeup as honest
   * subsets of the prompt and completion totals (writeup prompt =
   * `prompt_tokens - research_prompt_tokens`). Zero on days without
   * `tool_use_enabled` chat traffic. Backend column lives in
   * `usage_daily` (see `migrations/20260519000004_research_tokens.sql`).
   */
  research_prompt_tokens: number
  /** Research-phase completion-token share of `completion_tokens`. */
  research_completion_tokens: number
  request_count: number
}

export interface CourseMember {
  user_id: string
  eppn: string | null
  display_name: string | null
  role: string
  added_at: string
  /**
   * Per-course study pipeline stage for this member, populated only
   * when the course's `study_mode` flag is on. Drives the "Study"
   * column + the "Remove from study" button gating in the members
   * tab. Undefined when study mode is off OR the member has never
   * landed on the consent screen.
   */
  study_stage?: "consent" | "pre_survey" | "task" | "post_survey" | "done"
}

export interface RoleSuggestion {
  id: string
  user_id: string
  eppn: string | null
  display_name: string | null
  current_role: string | null
  suggested_role: string
  source: string
  source_detail: { lti_roles?: string[] } | null
  created_at: string
}

export interface Conversation {
  id: string
  course_id: string
  title: string | null
  pinned: boolean
  created_at: string
  updated_at: string
  /**
   * True when this conversation's owner (the student) has a
   * teacher note that arrived after their last view. Drives
   * the unread dot in the chat sidebar's conversation row.
   * Optional because not every endpoint that returns a
   * Conversation populates it (e.g. the pin endpoint, which is
   * teacher-side and doesn't render this field).
   */
  has_unread_note?: boolean
}

export interface ConversationWithUser extends Conversation {
  user_id: string
  user_eppn: string | null
  user_display_name: string | null
  message_count: number | null
  /** Only present on the /all endpoint (teacher view). */
  feedback_up?: number
  feedback_down?: number
  /**
   * Thumbs-down feedback rows that have neither been explicitly
   * acknowledged nor have a teacher note attached to the same
   * message. Either clearing rule (ack or note) drops the row
   * out of this counter; the dashboard's "Flagged" tab uses
   * this count to badge the conversation.
   */
  unaddressed_down?: number
  /**
   * True when there's student activity (a new user turn) the
   * teaching team hasn't seen since the last review. Drives the
   * dashboard's "Unreviewed" tab + per-row dot. Per the product
   * call, opening the conversation in the dashboard counts as a
   * review (read == reviewed) and clears this; explicit re-review
   * is just re-opening. Course-shared (any teacher / TA / owner /
   * admin clears it for the whole team).
   */
  teacher_unreviewed?: boolean
  /**
   * Timestamp of the most-recent teaching-team review, or null
   * when nobody on the team has opened this conversation since
   * the migration backfill ran. Surfaced as "Reviewed by X · 2d
   * ago" in the dashboard row.
   */
  last_reviewed_at?: string | null
  last_reviewed_by?: string | null
  last_reviewer_display_name?: string | null
}

export interface CourseFeedbackStats {
  total_up: number
  total_down: number
  categories: { category: string | null; count: number }[]
}

export interface TeacherNote {
  id: string
  conversation_id: string
  message_id: string | null
  author_id: string
  author_display_name: string | null
  content: string
  created_at: string
  updated_at: string
}

export interface ConversationDetail {
  messages: Message[]
  notes: TeacherNote[]
  feedback: MessageFeedback[]
  /**
   * Extraction-guard flag log. Empty for non-teacher viewers (the
   * backend gates this for privacy; a student viewing their own
   * conversation doesn't need to see "you tripped the guard at
   * turn 3" metadata; the rewrite already surfaced the visible
   * policy note to them). Ordered oldest-first, aligned to user
   * messages via `turn_index`.
   */
  flags: ConversationFlag[]
  /**
   * Aegis prompt-coaching analyses, one per user message that the
   * analyzer scored. Empty when aegis is off for the course or
   * every turn so far soft-failed. Ordered oldest-first to align
   * with `messages`.
   */
  prompt_analyses: PromptAnalysis[]
}

/**
 * One aegis verdict for a user prompt. The analyzer produces 0..=3
 * actionable suggestions (NOT scores) about how to improve the
 * draft; an empty array is a legitimate "looks good, nothing to
 * suggest" signal. Mirrors the backend wire shape from
 * `chat::AegisAnalysisPayload`.
 *
 * Used in two places:
 *   * Live; returned by `POST /aegis/analyze` while the student
 *     types and on Send (drives the right-rail panel + the
 *     just-in-time intercept dialog).
 *   * Persisted; attached to `ConversationDetail.prompt_analyses`
 *     for the History list.
 *
 * `id` and `created_at` are present on persisted rows from the
 * conversation-detail route and absent on live verdicts; both
 * fields are typed optional so one shape covers both.
 */
export interface PromptAnalysis {
  /** Set on persisted rows; undefined on live verdicts. */
  id?: string
  /** Set on persisted rows; undefined on live verdicts. */
  message_id?: string
  /**
   * 0..=2 suggestions, most-impactful first. Empty = the analyzer
   * found nothing worth suggesting; the panel renders an
   * affirmation rather than nothing.
   *
   * The cap dropped from 3 to 2 in the post-pilot rework: testers
   * reported three ideas for one prompt felt overwhelming and
   * read as grading rather than coaching.
   */
  suggestions: AegisSuggestion[]
  /** "beginner" | "expert"; which calibration produced this verdict. */
  mode: "beginner" | "expert"
  /**
   * Cerebras model that produced the verdict. The first fire of a
   * fresh draft runs on the cheap model; from the second fire
   * onward (once the analyzer has at least one verdict for this
   * draft) the server escalates to a higher-quality model that
   * follows the already-addressed-check section reliably. Echoed
   * back with the message body on Send so the persisted History
   * row reflects the actual runtime model. Optional for backward
   * compatibility with persisted rows from before the field landed.
   */
  model_used?: string
  /** Set on persisted rows; undefined on live verdicts. */
  created_at?: string
}

export interface AegisSuggestion {
  /**
   * Short tag the panel uses for grouping / iconography. Mapped
   * to the literature rubric (Clarity / Rationale / Audience /
   * Format / Tasks / Instruction / Examples / Constraints). The
   * type stays `string` so a server-side enum extension doesn't
   * force a frontend release; unknown kinds fall back to the raw
   * string via i18next's `defaultValue`.
   */
  kind: string
  /**
   * Importance: "high" | "medium" | "low". Drives the panel
   * card's per-suggestion colour (rose / amber / sky) so the
   * student sees which suggestions move the needle vs which are
   * polish. Old persisted rows without this field render as
   * "medium" via the migration backfill.
   */
  severity: string
  /** Single-sentence actionable improvement, second-person. */
  text: string
  /**
   * One to two sentences expanding on WHY the fix matters and what
   * the student should consider when applying it. Hidden behind
   * click-to-expand on the panel; the collapsed default just shows
   * `text`. Optional because persisted rows from before the field
   * landed deserialise without it.
   */
  explanation?: string
  /**
   * 3-4 candidate dropdown answers the analyzer produced. The
   * Review tray renders these as a `<Select>` (plus a trailing
   * "Other..." entry that opens a free-text input); the student's
   * pick rides into `answer` on the rewrite request. Optional and
   * defaults to empty for persisted rows from before the field
   * landed; the tray shows only the free-text input in that case
   * so historical suggestions stay reviewable.
   */
  options?: string[]
  /**
   * The student's chosen answer for this suggestion, set only on
   * the rewrite request body (the analyzer never returns it). The
   * banner tracks a per-suggestion answer in component state and
   * stamps it onto each suggestion when it calls `/aegis/rewrite`.
   */
  answer?: string
}

export interface ConversationFlag {
  id: string
  flag: string
  /**
   * 1-based index into the conversation's user-message stream.
   * Lets the per-turn UI on the conversation detail page align
   * the flag badge to the assistant message that followed the
   * flagged user input. Nullable because the flag schema is
   * generic; future flag kinds may not be turn-scoped.
   */
  turn_index: number | null
  rationale: string | null
  metadata: Record<string, unknown> | null
  created_at: string
  /**
   * NULL until a teacher clicks "Acknowledge" on the dashboard.
   * Acked flags still render in the conversation detail (audit
   * trail) but stop driving the per-row badge and stop pulling
   * the conversation into the "Needs Review" tab. Fixes the
   * prior "extraction flags are stuck forever" behaviour.
   */
  acknowledged_at?: string | null
  acknowledged_by?: string | null
  acknowledger_display_name?: string | null
}

export const FEEDBACK_CATEGORIES = [
  { value: "incorrect", label: "Incorrect or misleading" },
  { value: "off-topic", label: "Off-topic / not about the course" },
  { value: "incomplete", label: "Incomplete answer" },
  { value: "unclear", label: "Hard to understand" },
  { value: "harmful", label: "Harmful or inappropriate" },
  { value: "other", label: "Other" },
] as const

/**
 * Per-(category, model) aggregate of token spend over a window
 * (the backend currently returns a 30-day rolling window). The
 * dashboard sums these across categories for a "total spend" line
 * and shows the per-category breakdown as a small table.
 */
export interface KgTokenUsageRow {
  category: string
  model: string
  call_count: number
  prompt_tokens: number
  completion_tokens: number
}

export interface KgTokenUsage {
  /** ISO-8601 timestamp of the window start. */
  since: string
  rows: KgTokenUsageRow[]
}

export interface MessageFeedback {
  id: string
  message_id: string
  user_id: string
  rating: "up" | "down"
  category: string | null
  comment: string | null
  created_at: string
  updated_at: string
  user_eppn: string | null
  user_display_name: string | null
  /**
   * NULL until a teacher explicitly clicks "Mark as reviewed"
   * on this feedback row. Orthogonal to the legacy "leaving a
   * note on the same message addresses the downvote" path; the
   * dashboard's unaddressed_down counter ORs the two clearing
   * rules so either resolves it.
   */
  acknowledged_at?: string | null
  acknowledged_by?: string | null
  acknowledger_display_name?: string | null
}

export interface Message {
  id: string
  role: "user" | "assistant"
  content: string
  chunks_used: string[] | null
  model_used: string | null
  tokens_prompt: number | null
  tokens_completion: number | null
  generation_ms: number | null
  retrieval_count: number | null
  /**
   * Persisted research-phase output for assistant messages produced
   * by a `tool_use_enabled` course. NULL on legacy single-pass
   * messages; the chat UI hides the "Thinking" disclosure when both
   * are missing.
   */
  thinking_transcript: string | null
  tool_events: PersistedToolEvent[] | null
  /**
   * Research-phase wall-clock duration in milliseconds. Lets the
   * "Thinking" disclosure render "Thought for Ns" on past messages,
   * symmetrical with the live-stream timer.
   */
  thinking_ms: number | null
  /**
   * Research-phase prompt-token share of `tokens_prompt`. When
   * non-null the per-message footer can break the prompt total
   * into `research + writeup`. `null` on legacy single-pass
   * assistant messages and on user messages.
   */
  research_prompt_tokens: number | null
  /**
   * Research-phase completion-token share of `tokens_completion`.
   * Same null semantics as `research_prompt_tokens`.
   */
  research_completion_tokens: number | null
  /**
   * True when the extraction guard's constraint was active for this
   * turn. Server reads it from the `messages.thinking_hidden` column
   * and ORs in pre-migration historical signals (a turn with an
   * `extraction_rewrote` flag) for the conversation owner. Frontend
   * renders the disclosure as a placeholder when true: the
   * `thinking_transcript` and `tool_events` fields are blanked out
   * on the server side for the owner, so the disclosure component
   * keys off this flag rather than transcript-presence to decide
   * whether to render "[Reasoning hidden under integrity guard]".
   */
  thinking_hidden: boolean
  created_at: string
}

export interface TopicGroup {
  topic: string
  conversation_count: number
  unique_users: number
  total_messages: number
  conversation_ids: string[]
}

export interface ApiKey {
  id: string
  name: string
  key_prefix: string
  created_at: string
  last_used_at: string | null
}

/**
 * One tool call persisted on an assistant message. Mirrors the
 * `tool_call` + `tool_result` SSE event pair the chat stream emits
 * during a tool-use research phase, collapsed into a single
 * structured row. Stored as JSONB on the backend side.
 */
export interface PersistedToolEvent {
  name: string
  args?: unknown
  result_summary?: string
  /**
   * Raw JSON payload the tool returned to the model (already
   * truncated server-side). Frontend renders it click-to-expand on
   * each tool-call row.
   */
  result?: unknown
}

export interface PlayCourseCatalogEntry {
  code: string
  name: string
  updated_at: string
}

export interface PlayDesignation {
  id: string
  designation: string
  created_at: string
  last_synced_at: string | null
  last_error: string | null
}

export interface ApiKeyCreated {
  id: string
  name: string
  key: string
  key_prefix: string
  created_at: string
}

export interface MoodleToolConfig {
  tool_url: string
  lti_version: string
  public_key_type: string
  public_keyset_url: string
  initiate_login_url: string
  redirection_uris: string
  custom_parameters: string
  default_launch_container: string
  icon_url: string
  share_name: boolean
  share_email: boolean
  accept_grades: boolean
}

export interface LtiSetup {
  moodle_tool_config: MoodleToolConfig
  /**
   * Step-by-step manual setup. Used as the fallback when the LMS doesn't
   * support LTI Dynamic Registration, or when the admin clicks "manual
   * setup" on the site-wide page.
   */
  steps: string[]
  /**
   * Present on the site-wide admin response: pasting this URL into the
   * LMS's "configure tool by URL" flow auto-installs the tool with the
   * correct privacy, scopes, claims, and custom parameters. Absent on
   * the per-course response (per-course flows can't use dynreg).
   */
  dynamic_registration_url?: string
}

export interface LtiRegistration {
  id: string
  course_id: string
  name: string
  issuer: string
  client_id: string
  deployment_id: string | null
  auth_login_url: string
  auth_token_url: string
  platform_jwks_url: string
  created_at: string
  moodle_config: MoodleToolConfig
}

export interface LtiPlatform {
  id: string
  name: string
  issuer: string
  client_id: string
  deployment_id: string | null
  auth_login_url: string
  auth_token_url: string
  platform_jwks_url: string
  created_at: string
  moodle_config: MoodleToolConfig
  /// Empty array = platform can launch for any claimed eppn. See backend
  /// `enforce_platform_eppn_domain` for matching rules.
  allowed_eppn_domains: string[]
  /// null = pending approval (installed via Dynamic Registration; will not
  /// authenticate launches until an integrator clicks Approve). Non-null =
  /// active, launches and NRPS work normally.
  activated_at: string | null
  /// Non-null = the LMS has been continuously rejecting our
  /// client_credentials since this timestamp ("invalid_client" or 401/404
  /// from the token endpoint). Worker auto-deletes after 30 days; until
  /// then the admin UI flags the row for manual review.
  invalid_client_since: string | null
  /// Most recent platform-health probe bucket: "ok" | "invalid_client" |
  /// "http_<code>" | "network" | "parse_error". null = never probed yet.
  last_health_check_status: string | null
  last_health_check_at: string | null
}

export interface LtiNrpsStatus {
  id: string
  course_id: string
  /// "registration" (per-course) or "platform" (site-level).
  source: "registration" | "platform"
  context_id: string
  last_sync_at: string | null
  last_sync_status: "ok" | "error" | null
  last_sync_error: string | null
  /**
   * Independent of last_sync_status. A sync can be "ok" and still carry an
   * actionable note (e.g. the LMS isn't sharing identity claims for any
   * member, so they were all added with synthetic ids).
   */
  last_sync_warning: string | null
  last_sync_added: number | null
  last_sync_removed: number | null
}

export interface LtiPlatformBinding {
  id: string
  platform_id: string
  context_id: string
  context_label: string | null
  context_title: string | null
  course_id: string
  course_name: string | null
  created_at: string
}

/// A per-course `lti_registrations` row that's getting launches from
/// more than one LMS context_id. The Minerva-side scope is per-course,
/// but the LMS-side install is functioning as site-level: every launch
/// from every LMS course funnels into the SAME Minerva course. Almost
/// always a setup mistake; fix is to convert the registration into a
/// site-level platform + bindings, or to split it into separate
/// registrations per LMS context.
export interface LtiOverscopedRegistration {
  registration_id: string
  registration_name: string
  issuer: string
  client_id: string
  minerva_course_id: string
  minerva_course_name: string
  /// All distinct LMS context_id values observed so far (>= 2).
  observed_context_ids: string[]
  /// Earliest NRPS context creation time for any observed context; a
  /// rough "since when has this been wrong" signal.
  first_observed_at: string
}

export interface LtiDiagnostics {
  overscoped_registrations: LtiOverscopedRegistration[]
}

/// Site-level platform binding as seen from the course/teacher angle: same
/// (platform, context) -> course link, but joined with the platform metadata
/// the teacher actually cares about (name, issuer, client_id) since they
/// can't query the platform admin endpoints themselves.
export interface LtiCourseSiteBinding {
  id: string
  platform_id: string
  platform_name: string
  platform_issuer: string
  platform_client_id: string
  context_id: string
  context_label: string | null
  context_title: string | null
  created_at: string
}

export interface SiteIntegrationKey {
  id: string
  name: string
  key_prefix: string
  created_at: string
  last_used_at: string | null
  /// Empty array means the key can act for any eppn.
  allowed_eppn_domains: string[]
}

export interface SiteIntegrationKeyCreated {
  id: string
  name: string
  key: string
  key_prefix: string
  created_at: string
  allowed_eppn_domains: string[]
}

export interface LtiBindInfo {
  platform_name: string
  context_id: string
  context_label: string | null
  context_title: string | null
  is_teacher_role: boolean
  courses: { id: string; name: string }[]
}

export interface CanvasConnection {
  id: string
  course_id: string
  name: string
  canvas_base_url: string
  canvas_course_id: string
  auto_sync: boolean
  created_at: string
  updated_at: string
  last_synced_at: string | null
}

export type CanvasItemKind = "file" | "page" | "url"

export interface CanvasItemInfo {
  id: string
  filename: string
  kind: CanvasItemKind
  content_type: string | null
  size: number
  /** "files_api" and/or "modules": which discovery source surfaced the item. */
  sources: string[]
  already_synced: boolean
  needs_resync: boolean
}

export interface CanvasItemsResponse {
  items: CanvasItemInfo[]
  warnings: LocalizedMessage[]
}

export interface CanvasSyncResult {
  synced: number
  resynced: number
  skipped: number
  errors: LocalizedMessage[]
  warnings: LocalizedMessage[]
}

export interface ExternalAuthInvite {
  id: string
  jti: string
  eppn: string
  display_name: string | null
  created_at: string
  expires_at: string
  revoked_at: string | null
}

export interface ExternalAuthInviteCreated extends ExternalAuthInvite {
  /// Single-use callback URL. Only returned at creation; the raw token cannot
  /// be retrieved later; if the admin loses it, revoke and re-mint.
  url: string
}

export interface SystemMetrics {
  disk: {
    path: string
    total_bytes: number
    free_bytes: number
    used_bytes: number
  } | null
  database: {
    size_bytes: number | null
    table_counts: { name: string; rows: number }[]
  }
  documents: {
    count: number
    total_bytes: number
    pending: number
    failed: number
  }
  qdrant: {
    reachable: boolean
    collections: {
      name: string
      points_count: number | null
      indexed_vectors_count: number | null
      segments_count: number | null
    }[]
  }
}

export type DocumentKind =
  | "lecture"
  | "lecture_transcript"
  | "reading"
  | "tutorial_exercise"
  | "assignment_brief"
  | "sample_solution"
  | "lab_brief"
  | "exam"
  | "syllabus"
  | "unknown"

export const DOCUMENT_KINDS: DocumentKind[] = [
  "lecture",
  "lecture_transcript",
  "reading",
  "tutorial_exercise",
  "assignment_brief",
  "sample_solution",
  "lab_brief",
  "exam",
  "syllabus",
  "unknown",
]

export interface Document {
  id: string
  course_id: string
  filename: string
  mime_type: string
  size_bytes: number
  status: "pending" | "processing" | "ready" | "failed"
  chunk_count: number
  error_msg: string | null
  displayable: boolean
  uploaded_by: string
  created_at: string
  processed_at: string | null
  // Course knowledge graph V1; nullable until classifier runs.
  kind: DocumentKind | null
  kind_confidence: number | null
  kind_rationale: string | null
  kind_locked_by_teacher: boolean
  classified_at: string | null
  // Slice 2: source-identity columns. `source_system` is "moodle" /
  // "canvas" for plugin uploads, "manual" for teacher-tagged UI
  // uploads, null for untagged. Teachers can edit `source_ref` on
  // null- and "manual"-system docs via PATCH; plugin-owned refs are
  // protected server-side (PATCH returns 4xx). `orphaned_at` is set
  // when the doc has been superseded or its upstream source deleted.
  source_system: string | null
  source_ref: string | null
  orphaned_at: string | null
}
