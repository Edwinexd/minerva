import { queryOptions } from "@tanstack/react-query"
import { api } from "./api"
import type { AdminUser, ApiKey, CanvasConnection, CanvasItemsResponse, Conversation, ConversationDetail, ConversationWithUser, CourseFeedbackStats, Course, CourseMember, Document, ExternalAuthInvite, LtiPlatform, LtiPlatformBinding, LtiRegistration, LtiSetup, PlayCourseCatalogEntry, PlayDesignation, RoleRule, RoleSuggestion, SiteIntegrationKey, SystemMetrics, TeacherNote, TopicGroup, UsageRecord, User } from "./types"

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
  /// Cached pair decisions whose endpoints have been re-classified
  /// since -- the linker will re-evaluate these on its next sweep.
  pending_pairs: number
  /// Classified docs that have never appeared in any cached pair
  /// yet (a brand-new upload between two relink sweeps).
  new_doc_count: number
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
