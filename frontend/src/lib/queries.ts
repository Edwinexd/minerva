import { queryOptions } from "@tanstack/react-query"
import { api } from "./api"
import type { Conversation, Course, CourseMember, Document, Message, User } from "./types"

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

export const conversationsQuery = (courseId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "conversations"],
    queryFn: () => api.get<Conversation[]>(`/courses/${courseId}/conversations`),
  })

export const conversationMessagesQuery = (courseId: string, conversationId: string) =>
  queryOptions({
    queryKey: ["courses", courseId, "conversations", conversationId],
    queryFn: () => api.get<Message[]>(`/courses/${courseId}/conversations/${conversationId}`),
  })
