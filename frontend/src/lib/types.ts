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
  embedding_provider: string
  embedding_model: string
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
}
