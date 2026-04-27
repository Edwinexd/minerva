-- Per-course toggle for the eureka-2 concept knowledge graph integration.
--
-- Two-layer gating: the `eureka` cargo feature on `minerva-server`
-- decides whether the integration is compiled in at all; this column
-- decides whether it's exposed for a given course at runtime. New
-- courses default to off so existing behaviour is unchanged; admins or
-- teachers can flip it on per course once the feature is wired into the
-- frontend.
ALTER TABLE courses
    ADD COLUMN concept_graph_enabled BOOLEAN NOT NULL DEFAULT FALSE;
