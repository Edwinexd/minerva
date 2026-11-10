//! Shared authorization guards for course-scoped routes.
//!
//! These checks were previously reimplemented per route module, and the
//! copies had drifted on two axes that are invisible at a call site: whether
//! an admin short-circuits before the course is loaded (so a bogus course id
//! answered 200 instead of 404 for admins) and whether course assistants
//! count as teachers. Both axes are explicit here.

use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;
use minerva_core::models::User;
use minerva_db::queries::workspaces::WorkspaceRow;

/// Site administrator only.
pub(crate) fn require_admin(user: &User) -> Result<(), AppError> {
    if !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// Site-wide LTI platforms and integration keys are managed by admins and
/// integrators alike; the integrator role exists precisely to delegate this
/// without granting full admin.
pub(crate) fn require_site_integrator(user: &User) -> Result<(), AppError> {
    if !user.role.can_manage_site_integrations() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// Whether course assistants count as teachers for a given check.
pub(crate) enum TeacherScope {
    /// Teachers and assistants (`is_workspace_teacher`).
    WithAssistants,
    /// Teachers only; assistants are rejected (`is_teacher_strict`).
    /// LTI registration uses this.
    Strict,
}

/// Load a course, 404ing when it does not exist.
pub(crate) async fn load_workspace(
    state: &AppState,
    workspace_id: Uuid,
) -> Result<WorkspaceRow, AppError> {
    minerva_db::queries::workspaces::find_by_id(&state.db, workspace_id)
        .await?
        .ok_or(AppError::NotFound)
}

/// Authorize the caller as course owner, admin, or teacher, and hand back
/// the course row.
///
/// The course is resolved before the role check, so a course id that does
/// not exist is a 404 for everyone including admins.
pub(crate) async fn require_workspace_teacher(
    state: &AppState,
    workspace_id: Uuid,
    user: &User,
    scope: TeacherScope,
) -> Result<WorkspaceRow, AppError> {
    let course = load_workspace(state, workspace_id).await?;
    if is_teacher_of(state, &course, user, scope).await? {
        Ok(course)
    } else {
        Err(AppError::Forbidden)
    }
}

/// Is the caller a teacher of this course? For handlers that vary their
/// response rather than rejecting. A course id that does not exist is still
/// a 404.
pub(crate) async fn is_workspace_teacher(
    state: &AppState,
    workspace_id: Uuid,
    user: &User,
    scope: TeacherScope,
) -> Result<bool, AppError> {
    let course = load_workspace(state, workspace_id).await?;
    is_teacher_of(state, &course, user, scope).await
}

/// Same check against a course row the caller already holds. The chat send
/// path resolves the course before it can do anything else, so going through
/// `require_workspace_teacher` there would re-fetch it by primary key on every
/// turn just to answer one bool.
pub(crate) async fn is_teacher_of(
    state: &AppState,
    course: &WorkspaceRow,
    user: &User,
    scope: TeacherScope,
) -> Result<bool, AppError> {
    if user.role.is_admin() || course.owner_id == user.id {
        return Ok(true);
    }
    Ok(match scope {
        TeacherScope::WithAssistants => {
            minerva_db::queries::workspaces::is_teacher(&state.db, course.id, user.id).await?
        }
        TeacherScope::Strict => {
            minerva_db::queries::workspaces::is_teacher_strict(&state.db, course.id, user.id)
                .await?
        }
    })
}

/// Authorize the caller as course owner or admin, and hand back the course
/// row. Course membership grants nothing here; teachers manage everything
/// else in a course, but only the owner archives it.
pub(crate) async fn require_workspace_owner(
    state: &AppState,
    workspace_id: Uuid,
    user: &User,
) -> Result<WorkspaceRow, AppError> {
    let course = load_workspace(state, workspace_id).await?;
    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(course)
}

/// Conversations in a credit transfer workspace belong to their author
/// alone. Cases carry applicants' syllabuses, often with personal data,
/// so there is no teacher review, pinning or cross-conversation analytics.
pub(crate) fn conversations_are_private(kind: &str) -> bool {
    minerva_core::models::WorkspaceKind::parse(kind)
        == Some(minerva_core::models::WorkspaceKind::CreditTransfer)
}

/// [`require_workspace_teacher`] for endpoints that read or act on other
/// people's conversations. Not found in kinds whose conversations are
/// private, whoever asks.
pub(crate) async fn require_conversation_reviewer(
    state: &AppState,
    workspace_id: Uuid,
    user: &User,
    scope: TeacherScope,
) -> Result<WorkspaceRow, AppError> {
    let workspace = require_workspace_teacher(state, workspace_id, user, scope).await?;
    if conversations_are_private(&workspace.kind) {
        return Err(AppError::NotFound);
    }
    Ok(workspace)
}
