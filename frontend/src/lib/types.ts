export interface User {
  id: string
  eppn: string
  display_name: string | null
  role: "student" | "teacher" | "admin"
  suspended?: boolean
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
  strategy: string
  daily_token_limit: number
  active: boolean
  created_at: string
  updated_at: string
}

export interface AdminUser {
  id: string
  eppn: string
  display_name: string | null
  role: string
  suspended: boolean
  created_at: string
  updated_at: string
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
}

export interface Message {
  id: string
  role: "user" | "assistant"
  content: string
  chunks_used: string[] | null
  model_used: string | null
  tokens_prompt: number | null
  tokens_completion: number | null
  created_at: string
}

export interface Document {
  id: string
  course_id: string
  filename: string
  mime_type: string
  size_bytes: number
  status: "pending" | "processing" | "ready" | "failed"
  chunk_count: number
  error_msg: string | null
  uploaded_by: string
  created_at: string
  processed_at: string | null
}
