use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    Student,
    Teacher,
    Admin,
}

impl UserRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Student => "student",
            Self::Teacher => "teacher",
            Self::Admin => "admin",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "admin" => Self::Admin,
            "teacher" => Self::Teacher,
            _ => Self::Student,
        }
    }

    pub fn is_teacher_or_above(&self) -> bool {
        matches!(self, Self::Teacher | Self::Admin)
    }

    pub fn is_admin(&self) -> bool {
        matches!(self, Self::Admin)
    }

    /// Numeric rank: Student=0, Teacher=1, Admin=2. Used by `max` and any
    /// caller wanting an additive "highest of N roles" without re-deriving
    /// the ordering. Bumping a role above Admin would require a new rank.
    pub fn rank(&self) -> u8 {
        match self {
            Self::Student => 0,
            Self::Teacher => 1,
            Self::Admin => 2,
        }
    }

    /// Highest of two roles. Used by the rule engine (additive merge across
    /// matching rules) and the auth path (additive merge of stored vs
    /// rule-derived role). Ties return `a`.
    pub fn max(a: Self, b: Self) -> Self {
        if a.rank() >= b.rank() {
            a
        } else {
            b
        }
    }

    /// Clamp this role to <= Teacher. Used by auth.rs to demote a stored
    /// Admin row when the eppn is no longer in `MINERVA_ADMINS` -- the env
    /// is the source of truth for admins, and a stale stored role would
    /// otherwise outlive the env removal forever.
    pub fn clamp_below_admin(self) -> Self {
        match self {
            Self::Admin => Self::Teacher,
            other => other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub eppn: String,
    pub display_name: Option<String>,
    pub role: UserRole,
    pub suspended: bool,
    pub role_manually_set: bool,
    pub owner_daily_token_limit: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOperator {
    Contains,
    NotContains,
    Regex,
    NotRegex,
}

impl RuleOperator {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Contains => "contains",
            Self::NotContains => "not_contains",
            Self::Regex => "regex",
            Self::NotRegex => "not_regex",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "contains" => Some(Self::Contains),
            "not_contains" => Some(Self::NotContains),
            "regex" => Some(Self::Regex),
            "not_regex" => Some(Self::NotRegex),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleRule {
    pub id: Uuid,
    pub name: String,
    pub target_role: UserRole,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleRuleCondition {
    pub id: Uuid,
    pub rule_id: Uuid,
    pub attribute: String,
    pub operator: RuleOperator,
    pub value: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleRuleWithConditions {
    #[serde(flatten)]
    pub rule: RoleRule,
    pub conditions: Vec<RoleRuleCondition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Course {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub owner_id: Uuid,
    pub context_ratio: f64,
    pub temperature: f64,
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_chunks: i32,
    pub daily_token_limit: i64,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CourseMemberRole {
    Student,
    Ta,
    Teacher,
}

impl CourseMemberRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Student => "student",
            Self::Ta => "ta",
            Self::Teacher => "teacher",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "teacher" => Self::Teacher,
            "ta" => Self::Ta,
            _ => Self::Student,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CourseMember {
    pub course_id: Uuid,
    pub user_id: Uuid,
    pub role: CourseMemberRole,
    pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentStatus {
    Pending,
    Processing,
    Ready,
    Failed,
}

impl DocumentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Processing => "processing",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "processing" => Self::Processing,
            "ready" => Self::Ready,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: Uuid,
    pub course_id: Uuid,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub status: DocumentStatus,
    pub chunk_count: i32,
    pub error_msg: Option<String>,
    pub uploaded_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: Uuid,
    pub course_id: Uuid,
    pub user_id: Uuid,
    pub title: Option<String>,
    pub pinned: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeacherNote {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub message_id: Option<Uuid>,
    pub author_id: Uuid,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: String,
    pub content: String,
    pub chunks_used: Option<serde_json::Value>,
    pub model_used: Option<String>,
    pub tokens_prompt: Option<i32>,
    pub tokens_completion: Option<i32>,
    pub created_at: DateTime<Utc>,
}
