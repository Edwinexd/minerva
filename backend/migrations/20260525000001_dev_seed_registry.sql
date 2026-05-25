-- Dev-mode-only registry of fixture rows.
--
-- The dev seeder (POST /admin/dev/seed + the `seed-dev` CLI bin) wipes its
-- own prior fixtures before re-inserting, so re-running the seeder gives a
-- deterministic state without disturbing anything a developer created by
-- hand. To do that without a `is_seed BOOLEAN` column on every table, every
-- insert the seeder makes also writes a (table_name, pk) tuple into this
-- table; wipe = "for each row in seeds, DELETE from table_name where id =
-- pk, in FK-respecting order".
--
-- The PK is stored as TEXT, not UUID, so a future seeded table whose PK is
-- not a UUID (composite keys, INT, etc.) can be tracked the same way; for
-- the UUID-PK tables the seeder currently touches the value is just the
-- canonical hex form.
--
-- This table is intentionally not gated by MINERVA_DEV_MODE at the schema
-- level (migrations run identically in prod) - the gating lives at the
-- route + CLI bin entry points. In a prod DB this table simply stays
-- empty, costing nothing.
CREATE TABLE seeds (
    table_name TEXT NOT NULL,
    pk         TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (table_name, pk)
);

CREATE INDEX idx_seeds_table_name ON seeds (table_name);
