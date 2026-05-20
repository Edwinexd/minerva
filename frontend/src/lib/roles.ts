import type { User } from "./types"

/** Site-wide role as stored on the user (mirrors the backend `UserRole`). */
export type SiteRole = User["role"]

/**
 * True for roles with full teacher capabilities: teacher, integrator, admin.
 * The integrator role is a superset of teacher, so anywhere the UI gates a
 * teacher capability it should accept integrator too.
 */
export function isTeacherOrAbove(role: SiteRole | undefined): boolean {
  return role === "teacher" || role === "integrator" || role === "admin"
}

/**
 * True for roles allowed to mint site-wide integration keys and manage
 * site-wide LTI platforms: integrator and admin. Mirrors the backend's
 * `UserRole::can_manage_site_integrations`. Gates the admin nav entry and the
 * LTI / integrations admin tabs for non-admins.
 */
export function canManageSiteIntegrations(role: SiteRole | undefined): boolean {
  return role === "integrator" || role === "admin"
}
