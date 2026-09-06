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
use minerva_db::queries::courses::CourseRow;

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
    /// Teachers and assistants (`is_course_teacher`).
    WithAssistants,
    /// Teachers only; assistants are rejected (`is_course_teacher_strict`).
    /// LTI registration uses this.
    Strict,
}

/// Load a course, 404ing when it does not exist.
pub(crate) async fn load_course(state: &AppState, course_id: Uuid) -> Result<CourseRow, AppError> {
    minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)
}

/// Authorize the caller as course owner, admin, or teacher, and hand back
/// the course row.
///
/// The course is resolved before the role check, so a course id that does
/// not exist is a 404 for everyone including admins.
pub(crate) async fn require_course_teacher(
    state: &AppState,
    course_id: Uuid,
    user: &User,
    scope: TeacherScope,
) -> Result<CourseRow, AppError> {
    let course = load_course(state, course_id).await?;
    if is_teacher_of(state, &course, user, scope).await? {
        Ok(course)
    } else {
        Err(AppError::Forbidden)
    }
}

/// Is the caller a teacher of this course? For handlers that vary their
/// response rather than rejecting. A course id that does not exist is still
/// a 404.
pub(crate) async fn is_course_teacher(
    state: &AppState,
    course_id: Uuid,
    user: &User,
    scope: TeacherScope,
) -> Result<bool, AppError> {
    let course = load_course(state, course_id).await?;
    is_teacher_of(state, &course, user, scope).await
}

/// Same check against a course row the caller already holds. The chat send
/// path resolves the course before it can do anything else, so going through
/// `require_course_teacher` there would re-fetch it by primary key on every
/// turn just to answer one bool.
pub(crate) async fn is_teacher_of(
    state: &AppState,
    course: &CourseRow,
    user: &User,
    scope: TeacherScope,
) -> Result<bool, AppError> {
    if user.role.is_admin() || course.owner_id == user.id {
        return Ok(true);
    }
    Ok(match scope {
        TeacherScope::WithAssistants => {
            minerva_db::queries::courses::is_course_teacher(&state.db, course.id, user.id).await?
        }
        TeacherScope::Strict => {
            minerva_db::queries::courses::is_course_teacher_strict(&state.db, course.id, user.id)
                .await?
        }
    })
}

/// Authorize the caller as course owner or admin, and hand back the course
/// row. Course membership grants nothing here; this is the guard for
/// course-configuration surfaces (keys, signed URLs, settings).
pub(crate) async fn require_course_owner(
    state: &AppState,
    course_id: Uuid,
    user: &User,
) -> Result<CourseRow, AppError> {
    let course = load_course(state, course_id).await?;
    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }
    Ok(course)
}
