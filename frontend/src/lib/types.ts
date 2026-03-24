export interface User {
  id: string
  eppn: string
  display_name: string | null
  role: "student" | "teacher" | "admin"
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
  active: boolean
  created_at: string
  updated_at: string
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
  created_at: string
  updated_at: string
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
