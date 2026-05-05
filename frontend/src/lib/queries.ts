import { queryOptions } from "@tanstack/react-query"
import { api } from "./api"
import type { AdminUser, ApiKey, CanvasConnection, CanvasItemsResponse, Conversation, ConversationDetail, ConversationWithUser, CourseFeedbackStats, Course, CourseMember, Document, ExternalAuthInvite, KgTokenUsage, LtiPlatform, LtiPlatformBinding, LtiRegistration, LtiSetup, PlayCourseCatalogEntry, PlayDesignation, RoleRule, RoleSuggestion, SiteIntegrationKey, StudyState, StudySurvey, SystemMetrics, TeacherNote, TopicGroup, UsageRecord, User } from "./types"

export const userQuery = queryOptions({
  queryKey: ["auth", "me"],
  queryFn: () => api.get<User>("/auth/me"),
})

export const coursesQuery = queryOptions({
  queryKey: ["courses"],
  queryFn: () => api.get<Course[]>("/courses"),
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
  // Poll while a benchmark is running so the row's speed populates
  // automatically once it finishes. The hook in the page swaps to
  // a faster interval when `running` is true.
  refetchInterval: 5000,
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

export const conversationsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "conversations"],
    queryFn: () => api.get<Conversation[]>(`/courses/${courseId}/conversations`),
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

export const adminLtiSetupQuery = queryOptions({
  queryKey: ["admin", "lti", "setup"],
  queryFn: () => api.get<LtiSetup>("/admin/lti/setup"),
  staleTime: Infinity,
})

export const adminLtiPlatformsQuery = queryOptions({
  queryKey: ["admin", "lti", "platforms"],
  queryFn: () => api.get<LtiPlatform[]>("/admin/lti/platforms"),
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
}

export interface AdminStudySurveyConfig {
  kind: string
  questions: AdminStudyQuestionConfig[]
  response_count: number
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
/// Researcher uses this to spot stalls and to reach individual
/// participants by eppn for follow-up. Real eppns + display names
/// (no pseudonymisation) so the export's `participant_id`-only
/// transcripts can be reconciled against the live roster.
export interface AdminStudyParticipantRow {
  user_id: string
  eppn: string | null
  display_name: string | null
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
