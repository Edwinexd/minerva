import type { LocalizedMessage } from "./use-api-error"

export interface User {
  id: string
  eppn: string
  display_name: string | null
  role: "student" | "teacher" | "admin"
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

export interface RoleRule {
  id: string
  name: string
  target_role: "student" | "teacher"
  enabled: boolean
  created_at: string
  updated_at: string
  conditions: RoleRuleCondition[]
}

export interface UsageRecord {
  user_id: string
  course_id: string
  date: string
  prompt_tokens: number
  completion_tokens: number
  embedding_tokens: number
  request_count: number
}

export interface CourseMember {
  user_id: string
  eppn: string | null
  display_name: string | null
  role: string
  added_at: string
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
}

export interface ConversationWithUser extends Conversation {
  user_id: string
  user_eppn: string | null
  user_display_name: string | null
  message_count: number | null
  /** Only present on the /all endpoint (teacher view). */
  feedback_up?: number
  feedback_down?: number
  /** Thumbs-down messages with no teacher note yet. Drives "Needs Review". */
  unaddressed_down?: number
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
   * 0..=3 suggestions, most-impactful first. Empty = the analyzer
   * found nothing worth suggesting; the panel renders an
   * affirmation rather than nothing.
   */
  suggestions: AegisSuggestion[]
  /** "beginner" | "expert"; which calibration produced this verdict. */
  mode: "beginner" | "expert"
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
  steps: string[]
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
}
