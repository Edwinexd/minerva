import { queryOptions } from "@tanstack/react-query"
import { api } from "./api"
import type { AdminUser, ApiKey, Conversation, ConversationDetail, ConversationWithUser, Course, CourseMember, Document, ExternalAuthInvite, LtiRegistration, LtiSetup, PlayCourseCatalogEntry, PlayDesignation, SystemMetrics, TeacherNote, TopicGroup, UsageRecord, User } from "./types"

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
