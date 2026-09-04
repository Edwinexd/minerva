-- Semantic conversation themes are now the only implementation, so the
-- opt-in gate is gone from the application. Drop any lingering per-course
-- or global rows so the admin UI does not keep offering a dead toggle.

DELETE FROM feature_flags WHERE flag = 'semantic_topics';
