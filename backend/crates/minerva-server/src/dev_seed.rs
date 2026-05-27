//! Dev-mode seed data.
//!
//! Pours a fixed cast of users, courses, memberships, documents,
//! conversations and messages into the live DB so a fresh dev clone (or a
//! branch DB that's been blown away with `scripts/dev-db.sh refresh`) has
//! something to log into and click around. Strictly opt-in: every entry
//! point that calls [`run_seed`] also asserts `MINERVA_DEV_MODE=true`, so
//! prod can never trigger this even with a malformed config.
//!
//! ## Wipe strategy
//!
//! Re-running the seeder must produce a deterministic state, but the
//! developer's hand-created rows must survive. We square that by tagging
//! every row the seeder inserts in the `seeds` registry table (one row per
//! `(table_name, pk)`); the wipe walks that registry in FK-respecting
//! order and `DELETE`s those exact rows before re-inserting. Anything the
//! developer made by hand isn't tagged, so it's left alone.
//!
//! Some tables (course_members, documents, conversations, messages) are
//! also reachable through `ON DELETE CASCADE` from `courses`. We still
//! register them individually so the seeder can also clean up "orphans"
//! (a row whose course was deleted by hand but whose tag stayed); the
//! tag-driven DELETE no-ops on the cascaded rows and removes the orphans.
//!
//! ## Embedding
//!
//! Seeded courses are pinned to a *local* fastembed provider (`local` /
//! `sentence-transformers/all-MiniLM-L6-v2`, 384-dim) so the worker can
//! actually embed the documents without an OpenAI key in your dev shell.
//! `upload_or_dedup` writes the source bytes under `{docs_path}/{course_id}/`
//! and inserts the row in `status = 'pending'`; the background worker then
//! claims it on the next tick.

use std::collections::HashSet;

use serde::Serialize;
use uuid::Uuid;

use crate::error::AppError;
use crate::routes::documents::upload_or_dedup;
use crate::state::AppState;

/// Per-table counts of what the seeder inserted, plus what the wipe step
/// cleaned up before it. Serialised verbatim to the admin endpoint
/// response so the UI can render a "seeded N users, N courses, ..." toast.
#[derive(Debug, Serialize)]
pub struct SeedReport {
    pub admin_eppn: String,
    pub admin_user_id: Uuid,
    pub users: usize,
    pub courses: usize,
    pub course_members: usize,
    pub documents: usize,
    pub conversations: usize,
    pub messages: usize,
    pub external_invites: usize,
    pub wiped: WipeReport,
}

/// Counts of rows actually deleted by the wipe step (i.e. registry tags
/// that pointed at a still-present row). Cascaded deletes don't bump
/// these counters; they're folded into the parent table's count.
#[derive(Debug, Default, Serialize)]
pub struct WipeReport {
    pub messages: u64,
    pub conversations: u64,
    pub documents: u64,
    pub course_members: u64,
    pub external_invites: u64,
    pub courses: u64,
    pub users: u64,
    pub course_dirs_removed: u64,
    pub qdrant_collections_removed: u64,
}

/// Order of tables when walking the `seeds` registry to wipe. Children
/// first, then parents, so FKs without `ON DELETE CASCADE` (and there
/// are some - `external_auth_invites.created_by` -> `users.id`, for
/// instance) don't block the parent delete. The course bytes-on-disk
/// dir and Qdrant collection are taken care of out-of-band before the
/// `courses` row goes; see [`wipe`].
const WIPE_TABLE_ORDER: &[&str] = &[
    "messages",
    "conversations",
    "documents",
    "course_members",
    "external_auth_invites",
    "courses",
    "users",
];

/// Run the destructive wipe + re-insert. The caller is expected to have
/// already verified `MINERVA_DEV_MODE = true`; we re-check anyway as
/// defense in depth (a stale CLI build pointed at the wrong env would
/// otherwise nuke production fixtures).
///
/// `admin_eppn` is the eppn of the human operator running the seeder.
/// They must already have a `users` row (i.e. they've logged in at least
/// once) - we use their id to set ownership on the two "admin-owned"
/// fixture courses and to enrol them as a student/teacher in one of the
/// other-owner courses.
pub async fn run_seed(state: &AppState, admin_eppn: &str) -> Result<SeedReport, AppError> {
    if !state.config.dev_mode {
        return Err(AppError::Forbidden);
    }

    let admin = minerva_db::queries::users::find_by_eppn(&state.db, admin_eppn)
        .await?
        .ok_or_else(|| {
            AppError::bad_request_with(
                "dev_seed.admin_eppn_unknown",
                [("eppn", admin_eppn.to_string())],
            )
        })?;
    let admin_id = admin.id;
    if !crate::auth::user_from_row(admin).role.is_admin() && !state.config.is_admin(admin_eppn) {
        // Defense in depth - the HTTP handler also gates on admin role,
        // but the CLI bin doesn't, so the bouncer has to live here too.
        // The DB-role check covers admins demoted out of MINERVA_ADMINS
        // who still have a `role='admin'` row; the env check covers
        // freshly-added admins who haven't logged in since the env
        // change.
        return Err(AppError::Forbidden);
    }

    let wiped = wipe(state).await?;

    // ---- Users -------------------------------------------------------
    //
    // Six fresh users covering the role variations a developer hits
    // most often in the wild. The admin themselves is NOT in this list -
    // they already exist (we just looked them up), and the fixture
    // courses key off the admin's own id for ownership / membership.
    let teacher =
        upsert_seed_user(state, "seed-teacher@dev.local", "Tess Teacher", "teacher").await?;
    let integrator = upsert_seed_user(
        state,
        "seed-integrator@dev.local",
        "Iris Integrator",
        "integrator",
    )
    .await?;
    let alice = upsert_seed_user(state, "seed-alice@dev.local", "Alice Student", "student").await?;
    let bob = upsert_seed_user(state, "seed-bob@dev.local", "Bob Student", "student").await?;
    let carol = upsert_seed_user(state, "seed-carol@dev.local", "Carol Student", "student").await?;
    let dan = upsert_seed_user(state, "seed-dan@dev.local", "Dan Student", "student").await?;
    let ext_guest =
        upsert_seed_user(state, "ext:seed-guest@dev.local", "Gigi Guest", "student").await?;

    // Make every seed user privacy-acknowledged so the chat UI doesn't
    // immediately put them through the disclosure modal. (Skipping this
    // makes manually clicking through every fixture user tedious.)
    for u in [teacher, integrator, alice, bob, carol, dan, ext_guest] {
        let _ = minerva_db::queries::users::acknowledge_privacy(&state.db, u).await?;
    }

    // ---- External-auth invite (so the ext: user is "real") -----------
    //
    // The ext user's row exists; the invite gives the operator a
    // callback URL they can paste in a private window to log in as the
    // guest without going through Shib. We expire it 60 days out.
    let invite_id = Uuid::new_v4();
    let jti = Uuid::new_v4();
    sqlx::query!(
        r#"INSERT INTO external_auth_invites
           (id, jti, eppn, display_name, created_by, expires_at)
           VALUES ($1, $2, $3, $4, $5, NOW() + INTERVAL '60 days')"#,
        invite_id,
        jti,
        "ext:seed-guest@dev.local",
        Some("Gigi Guest".to_string()),
        admin_id,
    )
    .execute(&state.db)
    .await?;
    track(state, "external_auth_invites", invite_id).await?;

    // ---- Courses -----------------------------------------------------
    //
    // Four courses spanning the configuration matrix the developer
    // most commonly needs to switch between:
    //
    //   1. "Intro Programming" - simple strategy, admin owns,
    //      students enrolled. The boring baseline.
    //   2. "Advanced Algorithms" - FLARE strategy, admin owns,
    //      students enrolled. Exercises the iterative-retrieval path.
    //   3. "Web Development" - FLARE + tool_use_enabled (agentic),
    //      `teacher` owns, admin enrolled as a student. Lets you see
    //      a tool-use course from the student angle without leaving
    //      your admin login.
    //   4. "Database Systems" - simple, `teacher` owns, admin NOT
    //      enrolled at all. Exercises the "course exists but I can't
    //      see it" code path (hidden in the course list).
    let intro = create_seed_course(
        state,
        SeedCourse {
            name: "Intro Programming (seed)",
            description: Some("Simple strategy. Owned by the calling admin."),
            owner_id: admin_id,
            strategy: "simple",
            tool_use_enabled: false,
        },
    )
    .await?;
    let algos = create_seed_course(
        state,
        SeedCourse {
            name: "Advanced Algorithms (seed)",
            description: Some("FLARE retrieval. Owned by the calling admin."),
            owner_id: admin_id,
            strategy: "flare",
            tool_use_enabled: false,
        },
    )
    .await?;
    let web = create_seed_course(
        state,
        SeedCourse {
            name: "Web Development (seed)",
            description: Some(
                "FLARE + tool use (agentic). Other-teacher-owned, admin is a student.",
            ),
            owner_id: teacher,
            strategy: "flare",
            tool_use_enabled: true,
        },
    )
    .await?;
    let db_sys = create_seed_course(
        state,
        SeedCourse {
            name: "Database Systems (seed)",
            description: Some("Simple strategy. Other-teacher-owned, admin not enrolled."),
            owner_id: teacher,
            strategy: "simple",
            tool_use_enabled: false,
        },
    )
    .await?;

    // ---- Course memberships -----------------------------------------
    //
    // The owner is always also a `teacher` member of their own course
    // (mirroring what `POST /api/courses` does at line 249 of
    // routes/courses.rs). Without that the home page + chat sidebar
    // (both membership-driven) show the course as missing even though
    // the user owns it. Easy to get wrong - I did get it wrong on the
    // first cut and the admin saw 1 of their 3 courses instead of 3.
    //
    // `db_sys` deliberately has zero overlap with the admin so the
    // "hidden course" case isn't a trick of role-promotion timing.
    let mut member_count = 0usize;
    for (course_id, role, user_id) in [
        // Intro: admin owns + is its teacher; students + TA enrolled.
        (intro, "teacher", admin_id),
        (intro, "student", alice),
        (intro, "student", bob),
        (intro, "student", carol),
        (intro, "ta", dan),
        // Algorithms: admin owns + is its teacher; smaller cohort.
        (algos, "teacher", admin_id),
        (algos, "student", alice),
        (algos, "student", bob),
        // Web: seed-teacher owns + is its teacher; admin in as
        // student, plus an ext: guest so the obfuscation path has
        // something to obfuscate.
        (web, "teacher", teacher),
        (web, "student", admin_id),
        (web, "student", carol),
        (web, "student", ext_guest),
        // DB sys: seed-teacher owns + is its teacher; admin
        // deliberately absent.
        (db_sys, "teacher", teacher),
        (db_sys, "student", bob),
        (db_sys, "student", dan),
    ] {
        minerva_db::queries::courses::add_member(&state.db, course_id, user_id, role).await?;
        // Composite-PK table; encode the pair as `course_id:user_id`
        // so the registry can target the right row at wipe time.
        track_composite(state, "course_members", &format!("{course_id}:{user_id}")).await?;
        member_count += 1;
    }

    // ---- Documents ---------------------------------------------------
    //
    // One short text doc per course so chat retrieval has something to
    // ground against. `upload_or_dedup` writes the bytes to disk + inserts
    // the row in status='pending'; the background worker will chunk and
    // embed them (local provider, no OpenAI key required) on its next
    // tick. The bodies are deliberately tiny so embedding finishes in
    // a few seconds even on a cold worker.
    let mut doc_count = 0usize;
    for (course_id, filename, body) in [
        (intro, "intro-syllabus.txt", FIXTURE_DOC_INTRO),
        (intro, "intro-week1.txt", FIXTURE_DOC_INTRO_WEEK1),
        (algos, "algos-syllabus.txt", FIXTURE_DOC_ALGOS),
        (web, "web-overview.txt", FIXTURE_DOC_WEB),
        (db_sys, "db-syllabus.txt", FIXTURE_DOC_DB),
    ] {
        let row = upload_or_dedup(
            state,
            course_id,
            filename,
            "text/plain",
            body.as_bytes(),
            admin_id, // attribution: the operator who ran the seed
            None,
            Some("dev_seed"),
            Some(filename), // unique-per-course (every fixture filename differs)
        )
        .await?;
        track(state, "documents", row.id).await?;
        doc_count += 1;
    }

    // ---- Conversations + messages -----------------------------------
    //
    // A handful of conversations spread across users so the teacher
    // conversations-page list isn't empty and the per-student sidebar
    // shows prior history on login. We hard-code the message text
    // rather than running the LLM; the goal is fixture coverage, not a
    // realistic chat transcript.
    let mut convo_count = 0usize;
    let mut msg_count = 0usize;
    for (course_id, user_id, user_msg, assistant_msg) in [
        // Admin gets one conversation per course they participate in, so
        // switching courses doesn't always greet them with an empty
        // sidebar. They own intro + algos and are enrolled in web; db_sys
        // is intentionally excluded.
        (
            intro,
            admin_id,
            "Show me the late submission policy.",
            "Late submissions lose 10% per day, capped at 50% off. Resubmissions allowed up to one week after the deadline.",
        ),
        (
            algos,
            admin_id,
            "What's the grading breakdown?",
            "40% weekly problem sets, 25% midterm, 35% final exam. Problem sets due each Sunday at 23:59.",
        ),
        (
            web,
            admin_id,
            "Which router does this course use?",
            "Tanstack Router on the frontend.",
        ),
        // Plus a few seed-student conversations so the teacher
        // dashboard for those courses has cross-user data to show.
        (
            intro,
            alice,
            "What does the syllabus say about late submissions?",
            "Late submissions lose 10% per day. See the syllabus document.",
        ),
        (
            intro,
            bob,
            "When is the first assignment due?",
            "Week 2, Friday at 23:59. The full schedule is in the syllabus.",
        ),
        (
            algos,
            alice,
            "Can you summarise the algorithms covered in week 1?",
            "Week 1 covers asymptotic analysis, master theorem, and divide-and-conquer.",
        ),
        (
            web,
            ext_guest,
            "Is this course taught in English?",
            "Yes - all materials and lectures are in English.",
        ),
    ] {
        let conv_id = Uuid::new_v4();
        minerva_db::queries::conversations::create(&state.db, conv_id, course_id, user_id).await?;
        track(state, "conversations", conv_id).await?;
        convo_count += 1;

        let user_msg_id = Uuid::new_v4();
        minerva_db::queries::conversations::insert_message(
            &state.db,
            user_msg_id,
            conv_id,
            "user",
            user_msg,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
        )
        .await?;
        track(state, "messages", user_msg_id).await?;
        msg_count += 1;

        let asst_msg_id = Uuid::new_v4();
        minerva_db::queries::conversations::insert_message(
            &state.db,
            asst_msg_id,
            conv_id,
            "assistant",
            assistant_msg,
            None,
            Some("seed-fixture"),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            None,
            None,
            None,
            None,
            None,
            false,
        )
        .await?;
        track(state, "messages", asst_msg_id).await?;
        msg_count += 1;
    }

    Ok(SeedReport {
        admin_eppn: admin_eppn.to_string(),
        admin_user_id: admin_id,
        users: 7, // teacher, integrator, alice, bob, carol, dan, ext_guest
        courses: 4,
        course_members: member_count,
        documents: doc_count,
        conversations: convo_count,
        messages: msg_count,
        external_invites: 1,
        wiped,
    })
}

/// Inserts (idempotently via `find_or_create_by_eppn`) a seed user and
/// records the row in the registry. Returns the user's id.
async fn upsert_seed_user(
    state: &AppState,
    eppn: &str,
    display_name: &str,
    role: &str,
) -> Result<Uuid, AppError> {
    let (row, _created) = minerva_db::queries::users::find_or_create_by_eppn(
        &state.db,
        eppn,
        Some(display_name),
        role,
        // Unlimited owner-cap for seed users so an over-zealous test
        // session doesn't hit the per-owner ceiling mid-demo.
        0,
    )
    .await?;
    // Force-set the role even when the row already existed (e.g. an
    // earlier seed run that picked a different role and the user has
    // since been demoted). The registry tags this row so re-running
    // the seeder will roll the value forward.
    if row.role != role {
        let _ = minerva_db::queries::users::update_role(&state.db, row.id, role).await?;
    }
    track(state, "users", row.id).await?;
    Ok(row.id)
}

/// Course config the seeder cares about. Everything else stays at the
/// SQL column defaults (model = gpt-oss-120b, temperature 0.3, etc.).
struct SeedCourse<'a> {
    name: &'a str,
    description: Option<&'a str>,
    owner_id: Uuid,
    strategy: &'a str,
    tool_use_enabled: bool,
}

/// Inserts a course with the requested config and registers the row.
/// Two steps because `courses::create` doesn't accept strategy /
/// tool_use_enabled / embedding overrides; we set those via `update`
/// immediately after creation. The brief window between is harmless -
/// the course is unreachable to anyone outside this function until
/// `track` records it.
async fn create_seed_course(state: &AppState, config: SeedCourse<'_>) -> Result<Uuid, AppError> {
    let course_id = Uuid::new_v4();
    let _row = minerva_db::queries::courses::create(
        &state.db,
        course_id,
        &minerva_db::queries::courses::CreateCourse {
            name: config.name.to_string(),
            description: config.description.map(|s| s.to_string()),
            owner_id: config.owner_id,
            daily_token_limit: 0, // unlimited per-student for seed
            // Seed leaves the AI knobs at SQL DEFAULT; the immediate
            // `update()` below overrides each seed course's policy
            // (model, strategy, tool_use, etc.) explicitly.
            model: None,
            temperature: None,
            context_ratio: None,
            max_chunks: None,
            min_score: None,
            strategy: None,
            tool_use_enabled: None,
            embedding_provider: None,
            embedding_model: None,
            system_prompt: None,
            // Dev seed predates the per-semester grouping; pin every
            // seeded course to a stable VT2026 label so the My Courses
            // page renders a single header instead of an Ad-hoc bucket.
            semester_label: "VT2026".to_string(),
        },
    )
    .await?;
    track(state, "courses", course_id).await?;

    minerva_db::queries::courses::update(
        &state.db,
        course_id,
        &minerva_db::queries::courses::UpdateCourse {
            name: None,
            description: None,
            context_ratio: None,
            temperature: None,
            model: None,
            system_prompt: None,
            max_chunks: None,
            min_score: None,
            strategy: Some(config.strategy.to_string()),
            tool_use_enabled: Some(config.tool_use_enabled),
            // Pin to local fastembed so the worker can embed without
            // an OPENAI_API_KEY in the dev shell. all-MiniLM-L6-v2 is
            // the smallest of the local options (384-dim, ~25 MB).
            embedding_provider: Some("local".to_string()),
            embedding_model: Some("sentence-transformers/all-MiniLM-L6-v2".to_string()),
            daily_token_limit: None,
            semester_label: None,
        },
    )
    .await?;

    Ok(course_id)
}

/// Record a single-UUID-PK row in the registry. Used by every helper
/// above to tag rows for the next wipe.
async fn track(state: &AppState, table: &str, pk: Uuid) -> Result<(), AppError> {
    sqlx::query!(
        "INSERT INTO seeds (table_name, pk) VALUES ($1, $2)
         ON CONFLICT (table_name, pk) DO NOTHING",
        table,
        pk.to_string(),
    )
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Record a row whose primary key is a composite (e.g. `course_members`
/// PK is `(course_id, user_id)`). The caller is responsible for
/// formatting the composite into a stable string the wipe code knows
/// how to split.
async fn track_composite(state: &AppState, table: &str, pk: &str) -> Result<(), AppError> {
    sqlx::query!(
        "INSERT INTO seeds (table_name, pk) VALUES ($1, $2)
         ON CONFLICT (table_name, pk) DO NOTHING",
        table,
        pk,
    )
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Destructive cleanup of every previous seed run. Walks the registry
/// in [`WIPE_TABLE_ORDER`] and deletes the rows it points at; before
/// `courses` go we also nuke the course's on-disk doc dir and Qdrant
/// collection so the next ingest cycle starts clean.
///
/// The seed registry rows themselves are removed last (per-table, in
/// the same step that deletes the underlying row) so a partial wipe
/// can be safely resumed by re-running.
async fn wipe(state: &AppState) -> Result<WipeReport, AppError> {
    let mut report = WipeReport::default();

    // Collect the course ids first so we can clean up bytes-on-disk +
    // Qdrant collections before the DB row goes (the row is what gives
    // us the course id; deleting it would lose the lookup).
    let course_ids: Vec<Uuid> = sqlx::query!("SELECT pk FROM seeds WHERE table_name = 'courses'",)
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .filter_map(|r| Uuid::parse_str(&r.pk).ok())
        .collect();

    for course_id in &course_ids {
        // Remove the course's docs directory. `remove_dir_all` is a
        // no-op on a missing dir, so a manually-cleared filesystem
        // doesn't break the wipe.
        let dir = format!("{}/{}", state.config.docs_path, course_id);
        if tokio::fs::try_exists(&dir).await.unwrap_or(false) {
            if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
                tracing::warn!("dev_seed: failed to remove docs dir {dir}: {e}");
            } else {
                report.course_dirs_removed += 1;
            }
        }

        // Look up the course's embedding_version so we can delete the
        // versioned Qdrant collection. If the row is already gone
        // (orphaned tag), we don't know the version and can't safely
        // delete a guessed collection name; skip silently.
        if let Ok(Some(row)) = minerva_db::queries::courses::find_by_id(&state.db, *course_id).await
        {
            let collection = if row.embedding_version <= 1 {
                format!("course_{course_id}")
            } else {
                format!("course_{course_id}_v{}", row.embedding_version)
            };
            match state.qdrant.delete_collection(&collection).await {
                Ok(_) => report.qdrant_collections_removed += 1,
                Err(e) => {
                    // 404 on the collection is normal (worker hadn't
                    // gotten round to creating it before the wipe);
                    // log as info, not warn.
                    tracing::info!(
                        "dev_seed: qdrant delete_collection({collection}) returned: {e}",
                    );
                }
            }
        }
    }

    // Walk the registry table-by-table. Each step deletes the target
    // rows by id-set, then removes the matching `seeds` rows. We split
    // the per-table queries by composite vs UUID so the SQL stays
    // straightforward.
    for table in WIPE_TABLE_ORDER {
        let pks: Vec<String> =
            sqlx::query_scalar!("SELECT pk FROM seeds WHERE table_name = $1", *table,)
                .fetch_all(&state.db)
                .await?;
        if pks.is_empty() {
            continue;
        }

        let deleted = match *table {
            "messages" => delete_by_uuid_pk(&state.db, "messages", &pks).await?,
            "conversations" => delete_by_uuid_pk(&state.db, "conversations", &pks).await?,
            "documents" => delete_by_uuid_pk(&state.db, "documents", &pks).await?,
            "course_members" => delete_course_members(&state.db, &pks).await?,
            "external_auth_invites" => {
                delete_by_uuid_pk(&state.db, "external_auth_invites", &pks).await?
            }
            "courses" => delete_by_uuid_pk(&state.db, "courses", &pks).await?,
            "users" => delete_by_uuid_pk(&state.db, "users", &pks).await?,
            other => {
                tracing::warn!("dev_seed: unknown table in WIPE_TABLE_ORDER: {other}");
                0
            }
        };

        // Drop the registry tags now that the underlying rows are gone
        // (or never existed). Doing this per-table keeps a partial
        // wipe resumable - if a later step fails, the registry only
        // remembers what we haven't gotten to yet.
        sqlx::query!("DELETE FROM seeds WHERE table_name = $1", *table,)
            .execute(&state.db)
            .await?;

        match *table {
            "messages" => report.messages = deleted,
            "conversations" => report.conversations = deleted,
            "documents" => report.documents = deleted,
            "course_members" => report.course_members = deleted,
            "external_auth_invites" => report.external_invites = deleted,
            "courses" => report.courses = deleted,
            "users" => report.users = deleted,
            _ => {}
        }
    }

    Ok(report)
}

/// `DELETE FROM <table> WHERE id::TEXT = ANY($1)` with the table name
/// validated against the allow-list above (no user-controlled input).
/// We cast `id` to TEXT rather than parsing every pk into a UUID array
/// up front so a stray non-UUID tag (future composite-key table, hand-
/// edited row) just produces zero matches instead of an outright error.
async fn delete_by_uuid_pk(
    db: &sqlx::PgPool,
    table: &str,
    pks: &[String],
) -> Result<u64, sqlx::Error> {
    // Pre-filter to parseable UUIDs so the cast in SQL never fails on
    // a malformed tag. Composite-PK tables wouldn't appear here
    // (they're routed to their own helper above), but a corrupted
    // registry row would still get caught.
    let uuids: HashSet<Uuid> = pks.iter().filter_map(|p| Uuid::parse_str(p).ok()).collect();
    if uuids.is_empty() {
        return Ok(0);
    }
    let uuid_vec: Vec<Uuid> = uuids.into_iter().collect();
    let sql = format!("DELETE FROM {table} WHERE id = ANY($1)");
    let result = sqlx::query(&sql).bind(&uuid_vec).execute(db).await?;
    Ok(result.rows_affected())
}

/// Composite-PK delete for `course_members`. Each tag is
/// `course_id:user_id`; we parse both halves and shove them through a
/// single VALUES list rather than firing one DELETE per row.
async fn delete_course_members(db: &sqlx::PgPool, pks: &[String]) -> Result<u64, sqlx::Error> {
    let mut course_ids: Vec<Uuid> = Vec::with_capacity(pks.len());
    let mut user_ids: Vec<Uuid> = Vec::with_capacity(pks.len());
    for pk in pks {
        let Some((c, u)) = pk.split_once(':') else {
            continue;
        };
        let (Ok(c), Ok(u)) = (Uuid::parse_str(c), Uuid::parse_str(u)) else {
            continue;
        };
        course_ids.push(c);
        user_ids.push(u);
    }
    if course_ids.is_empty() {
        return Ok(0);
    }
    let result = sqlx::query!(
        "DELETE FROM course_members
         WHERE (course_id, user_id) IN (
             SELECT * FROM UNNEST($1::UUID[], $2::UUID[])
         )",
        &course_ids,
        &user_ids,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

// -----------------------------------------------------------------
// Fixture document bodies. Short on purpose: the worker's embedding
// step scales linearly with chunk count and dev-stack embedding
// throughput is a small fraction of prod (single CPU worker, no
// batching warmup), so keeping each doc under ~800 chars means a
// fresh seed run finishes embedding in seconds rather than minutes.
// -----------------------------------------------------------------

const FIXTURE_DOC_INTRO: &str = "Intro Programming - Syllabus

This course introduces the fundamentals of programming using Python.
We meet twice a week for ten weeks. Each week has a short reading,
one programming assignment, and a quiz.

Grading: 50% assignments, 30% final project, 20% quizzes. Late
submissions lose 10 percent per day, capped at 50 percent off.
Resubmissions are allowed up to one week after the original deadline.

Office hours: Tuesdays 14-16 and Thursdays 10-12, in the lab on
floor 4. Email the instructor at least 24 hours ahead for an
appointment outside those slots.

Required tools: a laptop running Python 3.11 or newer, VS Code with
the Python extension, and a GitHub account.
";

const FIXTURE_DOC_INTRO_WEEK1: &str = "Intro Programming - Week 1

Goals: install Python and VS Code, write your first script, learn
about variables, basic types (int, float, str, bool), and how to
read user input.

Reading: chapter 1 of the course book (linked on the LMS).

Assignment: write a script that asks the user for their birth year,
computes their current age, and prints a friendly greeting. Submit
by Friday week 2 at 23:59. The submission goes through the LMS;
make sure the file is named `greeting.py`.

Tip: if Python complains about not being installed, double-check
that you ticked 'Add to PATH' during the Windows installer. On
macOS you can `brew install python@3.11`.
";

const FIXTURE_DOC_ALGOS: &str = "Advanced Algorithms - Syllabus

A second-year course on algorithm design and analysis. We cover
asymptotic analysis, divide and conquer, greedy algorithms, dynamic
programming, graph algorithms (BFS, DFS, shortest paths, MSTs), and
an introduction to NP-completeness.

Week 1: asymptotic notation (O, Theta, Omega), the master theorem,
and divide-and-conquer recurrences (mergesort, Strassen, closest
pair).

Week 2: more divide and conquer plus a first look at randomised
algorithms (quickselect, randomised quicksort).

Grading: weekly problem sets (40%), a midterm (25%), and a final
exam (35%). Problem sets are due each Sunday at 23:59.
";

const FIXTURE_DOC_WEB: &str = "Web Development - Course Overview

This course covers full-stack web development with TypeScript on
both the client and server side. We use React with Tanstack Router
on the frontend and Hono on the backend, with PostgreSQL as the
primary data store.

Topics: HTTP fundamentals, REST vs RPC, authentication patterns
(sessions, JWTs, OAuth2), the React component model, routing,
state management, async data with Tanstack Query, and deploying
to a small Linux VM.

The course is taught in English. Assignments are individual; the
final project (weeks 8-10) may be done in pairs.
";

const FIXTURE_DOC_DB: &str = "Database Systems - Syllabus

An introduction to relational databases, SQL, schema design, and
the basics of query optimisation. Labs use PostgreSQL throughout.

Topics: ER modelling, normalisation (1NF through BCNF), SQL DDL
and DML, transactions and isolation levels, indexes (B-trees and
hash), query plans, and a first taste of NoSQL (key-value, document,
column-family).

Assessment: weekly lab exercises (50%), one design project (20%),
and a written exam (30%). The lab exercises must be demoed to a TA
in person; sign-up sheets are posted on the LMS each Monday.
";
