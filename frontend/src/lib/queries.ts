import { queryOptions } from "@tanstack/react-query"
import { api } from "./api"
import type { AdminUser, ApiKey, CanvasConnection, CanvasItemsResponse, Conversation, ConversationDetail, ConversationWithUser, CourseFeedbackStats, Course, CourseMember, Document, ExternalAuthInvite, KgTokenUsage, LtiCourseSiteBinding, LtiDiagnostics, LtiNrpsStatus, LtiPlatform, LtiPlatformBinding, LtiRegistration, LtiSetup, MergeSuggestionGroup, PlayCourseCatalogEntry, PlayDesignation, RoleRule, RoleRuleAttributeValues, RoleSuggestion, SiteIntegrationKey, StudyState, StudySurvey, SystemMetrics, TeacherNote, TopicGroup, UsageRecord, User } from "./types"

export const userQuery = queryOptions({
  queryKey: ["auth", "me"],
  queryFn: () => api.get<User>("/auth/me"),
})

export const coursesQuery = queryOptions({
  queryKey: ["courses"],
  queryFn: () => api.get<Course[]>("/courses"),
})

/**
 * Admin-only course listing. Same shape as `coursesQuery` but includes
 * archived courses (the teacher-facing `/courses` route hides them), so
 * the admin panel can restore or merge them. Keyed separately so it
 * doesn't collide with the teacher/student `["courses"]` cache.
 */
export const adminCoursesQuery = queryOptions({
  queryKey: ["admin", "courses"],
  queryFn: () => api.get<Course[]>("/admin/courses"),
})

/**
 * Heuristic merge candidates: groups of active courses that share a
 * name or a base course code (e.g. SUPCOM / SUPCOM-HI / SUPCOM-DIST).
 * Drives the "Suggested merges" panel on the admin courses tab. Keyed
 * under ["admin", "courses", ...] so an admin-courses invalidation
 * (after a merge / archive) refreshes the suggestions too.
 */
export const adminMergeSuggestionsQuery = queryOptions({
  queryKey: ["admin", "courses", "merge-suggestions"],
  queryFn: () =>
    api.get<MergeSuggestionGroup[]>("/admin/courses/merge-suggestions"),
})

export const courseQuery = (id: string) =>
  queryOptions({
    queryKey: ["courses", id],
    queryFn: () => api.get<Course>(`/courses/${id}`),
  })

export const courseMembersQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "members"],
    queryFn: () => api.get<CourseMember[]>(`/courses/${courseId}/members`),
  })

export const courseRoleSuggestionsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "role-suggestions"],
    queryFn: () =>
      api.get<RoleSuggestion[]>(`/courses/${courseId}/role-suggestions`),
  })

export const courseDocumentsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "documents"],
    queryFn: () => api.get<Document[]>(`/courses/${courseId}/documents`),
    refetchInterval: 5000, // Poll for processing status updates
  })

export const modelsQuery = queryOptions({
  queryKey: ["models"],
  queryFn: () => api.get<{ models: { id: string; name: string }[] }>("/models"),
  staleTime: Infinity,
})

export interface EmbeddingBenchmark {
  model: string
  dimensions: number
  embeddings_per_second: number
  total_ms: number
  corpus_size: number
}

export const embeddingBenchmarksQuery = queryOptions({
  queryKey: ["embedding-benchmarks"],
  queryFn: () => api.get<{ benchmarks: EmbeddingBenchmark[] }>("/embedding-benchmarks"),
  staleTime: Infinity,
})

export interface AdminEmbeddingModel {
  model: string
  dimensions: number
  // Latest benchmark for this model, or null if it hasn't been run on
  // this server boot. Models in `STARTUP_BENCHMARK_MODELS` are
  // populated automatically; everything else only fills in after an
  // admin runs the benchmark from the system page.
  benchmark: EmbeddingBenchmark | null
  warmed_at_startup: boolean
  // Admin-managed picker policy. When false, teachers can't pick this
  // model in the per-course config dropdown; courses already on it
  // keep working. Toggled via `PUT /admin/embedding-models` with
  // `{model, enabled}` in the body.
  enabled: boolean
  // True for the single model new courses are created with. Set via
  // `PUT /admin/embedding-models/default` with `{model}` in the body.
  // Existing courses are not touched when the default flips; only
  // future POST /courses calls pick up the new value.
  is_default: boolean
  // Number of active local-provider courses currently using this
  // model. Surfaced so the admin can see the impact of disabling
  // (and walk through the migrate dialog on the courses page after).
  courses_using: number
}

export interface AdminEmbeddingModelsResponse {
  models: AdminEmbeddingModel[]
  // True if a benchmark is currently in flight on the server. Used
  // to disable every "Run benchmark" button while we wait, since
  // the backend serializes runs to keep peak RAM bounded.
  running: boolean
}

export const adminEmbeddingModelsQuery = queryOptions({
  queryKey: ["admin", "embedding-models"],
  queryFn: () => api.get<AdminEmbeddingModelsResponse>("/admin/embedding-models"),
  // Poll only while a benchmark is running so the row's speed populates
  // automatically once it finishes; idle otherwise.
  refetchInterval: (q) => (q.state.data?.running ? 1500 : false),
})

/// Public picker feed for the per-course teacher dropdown. Auth-gated
/// (any logged-in user can fetch it; the list itself is non-secret),
/// returns only `enabled` catalog entries with their dimensions and
/// latest benchmark. Replaces the previous hardcoded model list in
/// the teacher config page.
export interface PublicEmbeddingModel {
  model: string
  dimensions: number
  benchmark: EmbeddingBenchmark | null
}

export const embeddingModelsQuery = queryOptions({
  queryKey: ["embedding-models"],
  queryFn: () =>
    api.get<{ models: PublicEmbeddingModel[] }>("/embedding-catalog"),
  // Fresh-ish: a teacher reopening config after an admin re-enables
  // a model shouldn't have to hard-refresh.
  staleTime: 60_000,
})

// ── Re-ranker model catalog ────────────────────────────────────────

// Cross-encoder throughput: (query, passage) pairs scored per second.
// Distinct from the embedder's embeddings_per_second metric.
export interface RerankBenchmark {
  model: string
  pairs_per_second: number
  total_ms: number
  pairs: number
}

export interface AdminRerankerModel {
  model: string
  // Admin-managed picker policy. When false, teachers can't pick this
  // re-ranker in the per-course config dropdown; courses already on it
  // keep working. Toggled via `PUT /admin/reranker-models` with
  // `{model, enabled}`.
  enabled: boolean
  // True for the single re-ranker new courses default to. Set via
  // `PUT /admin/reranker-models/default` with `{model}`.
  is_default: boolean
  // Number of active courses currently using this re-ranker (no
  // provider filter; re-ranking applies regardless of embedding).
  courses_using: number
  // Latest benchmark, or null if not run since boot. Populated on
  // demand by the admin "Run benchmark" button.
  benchmark: RerankBenchmark | null
}

export interface AdminRerankerModelsResponse {
  models: AdminRerankerModel[]
  // True while a benchmark is in flight on the server (one model loaded
  // + scored at a time). Disables every "Run benchmark" button.
  running: boolean
}

export const adminRerankerModelsQuery = queryOptions({
  queryKey: ["admin", "reranker-models"],
  queryFn: () => api.get<AdminRerankerModelsResponse>("/admin/reranker-models"),
  // Poll only while a benchmark is running (matches the embedding query)
  // so the speed fills in when the run finishes; idle otherwise.
  refetchInterval: (q) => (q.state.data?.running ? 1500 : false),
})

/// Public picker feed for the per-course teacher re-ranker dropdown.
/// Returns only `enabled` catalog entries; carries just the id (the
/// frontend display map supplies friendly names + multilingual hints).
export interface PublicRerankerModel {
  model: string
}

export const rerankerModelsQuery = queryOptions({
  queryKey: ["reranker-models"],
  queryFn: () =>
    api.get<{ models: PublicRerankerModel[] }>("/reranker-catalog"),
  staleTime: 60_000,
})

export const conversationsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "conversations"],
    queryFn: () => api.get<Conversation[]>(`/courses/${courseId}/conversations`),
  })

/**
 * Cross-course unread-conversation counts for the calling user.
 * Returns `{course_id: count}` for any course with at least one
 * conversation that received a teacher note after the student
 * last viewed it; courses with zero unread are omitted server-side
 * so the payload stays tight. Drives the unread badge on the
 * "My Courses" tile.
 *
 * Modest staleTime: a fresh teacher note that just landed should
 * surface promptly, but we don't need realtime polling; the
 * student visiting a course will refetch on focus / mount anyway,
 * and the dot clearing happens via invalidation on mark-read.
 */
export const unreadCountsQuery = queryOptions({
  queryKey: ["courses", "unread-counts"],
  queryFn: () => api.get<Record<string, number>>(`/courses/unread-counts`),
  staleTime: 30 * 1000,
})

export const allConversationsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "conversations", "all"],
    queryFn: () => api.get<ConversationWithUser[]>(`/courses/${courseId}/conversations/all`),
  })

export const pinnedConversationsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "conversations", "pinned"],
    queryFn: () => api.get<ConversationWithUser[]>(`/courses/${courseId}/conversations/pinned`),
  })

export interface SuggestedQuestionsResponse {
  questions: string[]
}

/// Starter prompts for the chat empty state. Server-side cache is
/// the real bound on freshness; the 1h client stale-time just
/// keeps the empty-state snappy across rapid /new visits.
export const suggestedQuestionsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "suggested-questions"],
    queryFn: () =>
      api.get<SuggestedQuestionsResponse>(
        `/courses/${courseId}/suggested-questions`,
      ),
    staleTime: 60 * 60 * 1000,
  })

export const popularTopicsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "conversations", "topics"],
    queryFn: () => api.get<TopicGroup[]>(`/courses/${courseId}/conversations/topics`),
  })

export const courseFeedbackStatsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "feedback-stats"],
    queryFn: () => api.get<CourseFeedbackStats>(`/courses/${courseId}/conversations/feedback-stats`),
  })

/**
 * Per-conversation flag-kind map for the teacher conversation list.
 * Returns `{ conversationId: ["extraction_attempt", ...] }` so each
 * row can render badges without fetching the full flag log.
 * Teacher-only; backend rejects with 403 otherwise.
 */
export const conversationFlagKindsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "conversations", "flag-kinds"],
    queryFn: () =>
      api.get<Record<string, string[]>>(
        `/courses/${courseId}/conversations/flag-kinds`,
      ),
  })

export const conversationDetailQuery = (courseId: string, conversationId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "conversations", conversationId],
    queryFn: () => api.get<ConversationDetail>(`/courses/${courseId}/conversations/${conversationId}`),
  })

export const conversationNotesQuery = (courseId: string, conversationId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "conversations", conversationId, "notes"],
    queryFn: () => api.get<TeacherNote[]>(`/courses/${courseId}/conversations/${conversationId}/notes`),
  })

export const apiKeysQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "api-keys"],
    queryFn: () => api.get<ApiKey[]>(`/courses/${courseId}/api-keys`),
  })

export const playDesignationsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "play-designations"],
    queryFn: () => api.get<PlayDesignation[]>(`/courses/${courseId}/play-designations`),
  })

export const playCourseCatalogQuery = queryOptions({
  queryKey: ["play-courses-catalog"],
  queryFn: () => api.get<PlayCourseCatalogEntry[]>(`/play-courses-catalog`),
  staleTime: 5 * 60 * 1000,
})

export const ltiSetupQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "lti", "setup"],
    queryFn: () => api.get<LtiSetup>(`/courses/${courseId}/lti/setup`),
    staleTime: Infinity,
  })

export const ltiRegistrationsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "lti"],
    queryFn: () => api.get<LtiRegistration[]>(`/courses/${courseId}/lti`),
  })

export const ltiNrpsStatusQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "lti", "nrps"],
    queryFn: () => api.get<LtiNrpsStatus[]>(`/courses/${courseId}/lti/nrps`),
  })

export const ltiCourseSiteBindingsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "lti", "site-bindings"],
    queryFn: () =>
      api.get<LtiCourseSiteBinding[]>(`/courses/${courseId}/lti/site-bindings`),
  })

export const canvasConnectionsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "canvas"],
    queryFn: () => api.get<CanvasConnection[]>(`/courses/${courseId}/canvas`),
  })

export const canvasFilesQuery = (courseId: string, connectionId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "canvas", connectionId, "files"],
    queryFn: () =>
      api.get<CanvasItemsResponse>(`/courses/${courseId}/canvas/${connectionId}/files`),
  })

/**
 * Per-course KG / extraction-guard token spend, broken out per
 * (category, model) for the last 30 days. Distinct from the
 * existing per-student chat-token tracking; this is the cost the
 * course itself burned on classifier / linker / adversarial filter
 * / extraction guard. Teacher / owner / admin only.
 */
export const courseKgTokenUsageQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "kg-token-usage"],
    queryFn: () =>
      api.get<KgTokenUsage>(`/courses/${courseId}/kg-token-usage`),
  })

export const adminUsersQuery = queryOptions({
  queryKey: ["admin", "users"],
  queryFn: () => api.get<AdminUser[]>("/admin/users"),
})

export interface DaisyPendingParticipant {
  display_name: string | null
  /**
   * Resolved SU eppns for this participant, newest-first. The first
   * entry is the canonical primary; the rest land as
   * `user_eppn_aliases` when this row is applied.
   */
  eppns: string[]
  daisy_roles: string[]
  kind: string
}

export interface DaisyPendingImport {
  id: string
  momenttillf_id: string
  course_code: string
  name: string
  semester_label: string
  daisy_info_url: string | null
  daisy_syllabus_url: string | null
  daisy_unit: string | null
  participant_count: number
  participants: DaisyPendingParticipant[]
  /**
   * Null = brand-new offering (Apply will INSERT). Set = the
   * `courses.id` an Apply would refresh.
   */
  existing_course_id: string | null
  first_seen_at: string
  last_seen_at: string
}

export interface DaisyPendingListResponse {
  auto_apply: boolean
  auto_apply_updated_at: string
  auto_apply_updated_by: string | null
  pending: DaisyPendingImport[]
}

export const daisyPendingQuery = queryOptions({
  queryKey: ["admin", "daisy-pending"],
  queryFn: () => api.get<DaisyPendingListResponse>("/admin/daisy-pending"),
})

export const adminUsageQuery = queryOptions({
  queryKey: ["admin", "usage"],
  queryFn: () => api.get<UsageRecord[]>("/usage"),
})

export const externalAuthInvitesQuery = queryOptions({
  queryKey: ["admin", "external-invites"],
  queryFn: () => api.get<ExternalAuthInvite[]>("/admin/external-invites"),
})

export const adminSystemMetricsQuery = queryOptions({
  queryKey: ["admin", "system"],
  queryFn: () => api.get<SystemMetrics>("/admin/system"),
  refetchInterval: 30_000,
})

export interface BackfillProgress {
  started_at: string
  total: number
  ok: number
  errors: number
  skipped: number
  finished: boolean
}

export interface ClassificationStats {
  total_ready: number
  classified: number
  unclassified: number
  locked_by_teacher: number
  /// `null` until the admin has kicked off at least one backfill since
  /// the last server restart. While a backfill is running the polling
  /// /admin/classification-stats endpoint returns updated counters
  /// every 5s so the UI's progress bar ticks live.
  backfill: BackfillProgress | null
}

export const adminClassificationStatsQuery = queryOptions({
  queryKey: ["admin", "classification-stats"],
  queryFn: () => api.get<ClassificationStats>("/admin/classification-stats"),
  // While a backfill is running the unclassified counter ticks down
  // doc-by-doc; refresh often enough that the operator sees progress.
  refetchInterval: 5_000,
})

// ── Course knowledge graph ─────────────────────────────────────────

export interface KnowledgeGraphNode {
  id: string
  filename: string
  kind: string | null
  kind_confidence: number | null
  kind_locked_by_teacher: boolean
  chunk_count: number | null
}

export interface KnowledgeGraphEdge {
  /// Stable identifier; addressable as
  /// `/courses/{courseId}/documents/knowledge-graph/edges/{id}/reject`
  /// for per-edge teacher veto + un-veto.
  id: string
  src_id: string
  dst_id: string
  relation: "solution_of" | "part_of_unit" | "prerequisite_of" | "applied_in"
  confidence: number
  rationale: string | null
  rejected_by_teacher: boolean
}

export interface KnowledgeGraph {
  nodes: KnowledgeGraphNode[]
  edges: KnowledgeGraphEdge[]
  edges_computed: boolean
  /// True iff the linker has work waiting; either the course is
  /// queued for a relink sweep, or there are cached pair decisions
  /// whose endpoints have been re-classified since. Drives the
  /// "Linking..." pill on the graph viewer.
  linker_pending: boolean
}

export const courseKnowledgeGraphQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "knowledge-graph"],
    queryFn: () =>
      api.get<KnowledgeGraph>(`/courses/${courseId}/documents/knowledge-graph`),
    /// Poll while the linker's catching up; the pending indicators
    /// flip back to 0 once the next sweep tick lands and the
    /// teacher sees the graph stabilise without manually refreshing.
    refetchInterval: 8_000,
  })

export const adminRoleRulesQuery = queryOptions({
  queryKey: ["admin", "role-rules"],
  queryFn: () => api.get<RoleRule[]>("/admin/role-rules"),
})

/**
 * Suggestion list for the rule-condition value field, grouped per attribute.
 * The backend filters to values seen on at least `min_users` distinct users
 * (privacy guard against fishing for one specific person's attributes) and
 * orders each bucket by user count desc. Cached briefly so switching
 * attributes in the form is instant; observations only grow when users
 * actually log in, so freshness on the minute granularity is fine.
 */
export const adminRoleRuleAttributeValuesQuery = queryOptions({
  queryKey: ["admin", "role-rules", "attribute-values"],
  queryFn: () =>
    api.get<RoleRuleAttributeValues>("/admin/role-rules/attribute-values"),
  staleTime: 60 * 1000,
})

export const adminLtiSetupQuery = queryOptions({
  queryKey: ["admin", "lti", "setup"],
  queryFn: () => api.get<LtiSetup>("/admin/lti/setup"),
  staleTime: Infinity,
})

export const adminLtiPlatformsQuery = queryOptions({
  queryKey: ["admin", "lti", "platforms"],
  queryFn: () => api.get<LtiPlatform[]>("/admin/lti/platforms"),
})

/// Setup-health warnings across the LTI subsystem. Currently surfaces
/// per-course registrations that are receiving launches from more than
/// one LMS context (i.e. the LMS-side install is site-level but the
/// Minerva-side scope is per-course). Cheap join + grouping in pg; the
/// page polls only on mount, since these are config issues that don't
/// change without admin action.
export const adminLtiDiagnosticsQuery = queryOptions({
  queryKey: ["admin", "lti", "diagnostics"],
  queryFn: () => api.get<LtiDiagnostics>("/admin/lti/diagnostics"),
})

export const adminIntegrationKeysQuery = queryOptions({
  queryKey: ["admin", "integration-keys"],
  queryFn: () => api.get<SiteIntegrationKey[]>("/admin/integration-keys"),
})

export const adminLtiPlatformBindingsQuery = (platformId: string) =>
  queryOptions({
    queryKey: ["admin", "lti", "platforms", platformId, "bindings"],
    queryFn: () =>
      api.get<LtiPlatformBinding[]>(`/admin/lti/platforms/${platformId}/bindings`),
  })

export const adminLtiPlatformNrpsQuery = (platformId: string) =>
  queryOptions({
    queryKey: ["admin", "lti", "platforms", platformId, "nrps"],
    queryFn: () =>
      api.get<LtiNrpsStatus[]>(`/admin/lti/platforms/${platformId}/nrps`),
  })

// ── Study mode ─────────────────────────────────────────────────────

/// Participant pipeline state for one (course, viewer). Returned by
/// `GET /api/courses/{id}/study/state`. The study landing route
/// dispatches on `stage`. The route handler lazily inserts the
/// `study_participant_state` row on first hit, so a fresh course
/// member always lands in `consent`.
export const studyStateQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "study", "state"],
    queryFn: () => api.get<StudyState>(`/courses/${courseId}/study/state`),
    /// Don't cache aggressively: the participant moves between
    /// stages by mutation, and stale state would mean the wrong
    /// component renders for one tick.
    staleTime: 0,
  })

export const studySurveyQuery = (courseId: string, kind: "pre" | "post") =>
  queryOptions({
    queryKey: ["courses", courseId, "study", "survey", kind],
    queryFn: () => api.get<StudySurvey>(`/courses/${courseId}/study/survey/${kind}`),
  })

// ── Admin study config ─────────────────────────────────────────────

/// Full per-course study configuration as returned by
/// `GET /api/admin/study/courses/{id}/config`. The PUT endpoint
/// accepts the same shape (minus `course_id`, `has_in_flight_participants`,
/// `pre_survey.response_count`, `post_survey.response_count`).
export interface AdminStudyConfig {
  course_id: string
  number_of_tasks: number
  completion_gate_kind: string
  consent_html: string
  thank_you_html: string
  tasks: AdminStudyTask[]
  pre_survey: AdminStudySurveyConfig | null
  post_survey: AdminStudySurveyConfig | null
  has_in_flight_participants: boolean
}

export interface AdminStudyTask {
  task_index: number
  title: string
  description: string
  /**
   * Per-round Aegis support flag. FALSE hides the panel /
   * suppresses analyzer + rewrite calls for the round; TRUE
   * (the schema default) keeps it on. Used by the DM2731 design
   * to alternate support: rounds 1 and 3 off, round 2 on.
   */
  aegis_enabled: boolean
}

export interface AdminStudySurveyConfig {
  kind: string
  questions: AdminStudyQuestionConfig[]
  response_count: number
}

/// Body for PUT /admin/study/courses/{id}/config. Asymmetric with
/// `AdminStudyConfig`: GET returns surveys as `{kind, questions,
/// response_count}` objects (so the UI can show the response count
/// and the kind label); PUT expects bare `questions[]` arrays
/// because the kind is implied by which key (`pre_survey` /
/// `post_survey`) the array sits under, and `response_count` is
/// derived server-side, not editable.
export interface AdminStudyConfigPutBody {
  number_of_tasks: number
  completion_gate_kind: string
  consent_html: string
  thank_you_html: string
  tasks: AdminStudyTask[]
  pre_survey: AdminStudyQuestionConfig[]
  post_survey: AdminStudyQuestionConfig[]
}

export interface AdminStudyQuestionConfig {
  kind: "likert" | "free_text" | "section_heading"
  prompt: string
  likert_min: number | null
  likert_max: number | null
  likert_min_label: string | null
  likert_max_label: string | null
  is_required: boolean
  kill_on_value: number | null
}

export const adminStudyConfigQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["admin", "study", "courses", courseId, "config"],
    queryFn: () =>
      api.get<AdminStudyConfig>(`/admin/study/courses/${courseId}/config`),
  })

/// Per-participant progress for the admin "Participants" panel.
/// Anonymous on purpose: `participant_number` (assigned at consent
/// time, persistent) is the only identifier surfaced to researchers
/// during analysis. The "who is participant 5?" lookup happens
/// via the regular course members tab, where names live alongside
/// a `study_stage` field for matching.
export interface AdminStudyParticipantRow {
  /** NULL for pre-consent rows (consent screen drop-off). */
  participant_number: number | null
  stage: string
  current_task_index: number
  consented_at: string | null
  pre_survey_completed_at: string | null
  post_survey_completed_at: string | null
  locked_out_at: string | null
}

export const adminStudyParticipantsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["admin", "study", "courses", courseId, "participants"],
    queryFn: () =>
      api.get<AdminStudyParticipantRow[]>(
        `/admin/study/courses/${courseId}/participants`,
      ),
    /// Stale-while-revalidate is fine for this; the operator
    /// expects fresh-ish data but not realtime.
    staleTime: 10_000,
  })

/// Full per-participant data dump for the researcher's drill-in
/// view. Shape mirrors one line of the JSONL export. Keyed by
/// participant_number, never user_id.
export interface AdminStudyParticipantDetail {
  participant_number: number
  stage: string
  consented_at: string | null
  pre_survey_completed_at: string | null
  post_survey_completed_at: string | null
  locked_out_at: string | null
  pre_survey_responses: AdminStudySurveyResponse[]
  post_survey_responses: AdminStudySurveyResponse[]
  tasks: AdminStudyParticipantTask[]
}

export interface AdminStudySurveyResponse {
  question_id: string
  question_ord: number
  question_prompt: string
  question_kind: string
  likert_value: number | null
  free_text_value: string | null
  submitted_at: string
}

export interface AdminStudyParticipantTask {
  task_index: number
  task_title: string | null
  task_description: string | null
  conversation_id: string
  started_at: string
  marked_done_at: string | null
  messages: AdminStudyTaskMessage[]
  aegis_prompt_analyses: AdminStudyAegisAnalysis[]
  aegis_live_iterations: AdminStudyAegisIteration[]
}

export interface AdminStudyTaskMessage {
  id: string
  role: string
  content: string
  model_used: string | null
  tokens_prompt: number | null
  tokens_completion: number | null
  generation_ms: number | null
  retrieval_count: number | null
  created_at: string
}

export interface AdminStudyAegisAnalysis {
  message_id: string
  /** JSONB suggestions array; each suggestion is `{kind, severity, text, explanation, ...}`. */
  suggestions: unknown
  mode: string
  model_used: string
  created_at: string
}

export interface AdminStudyAegisIteration {
  id: string
  draft_text: string
  /** Same JSONB shape as analysis.suggestions. */
  suggestions: unknown
  mode: string
  model_used: string
  created_at: string
}

export const adminStudyParticipantDetailQuery = (
  courseId: string,
  participantNumber: number,
) =>
  queryOptions({
    queryKey: [
      "admin",
      "study",
      "courses",
      courseId,
      "participants",
      participantNumber,
      "detail",
    ],
    queryFn: () =>
      api.get<AdminStudyParticipantDetail>(
        `/admin/study/courses/${courseId}/participants/${participantNumber}/detail`,
      ),
  })
