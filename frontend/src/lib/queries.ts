import { queryOptions } from "@tanstack/react-query"
import { api } from "./api"
import type { Conversation, ConversationDetail, ConversationWithUser, Course, CourseMember, Document, TeacherNote, User } from "./types"

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
