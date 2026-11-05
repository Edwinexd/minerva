use minerva_db::queries::rulebook::{self as q, EntryInput, FileInput, PageRead};
use sqlx::PgPool;

const SECTION: &str = "https://medarbetare.su.se/vart-su/styrning/regelboken/utbildning";

fn page(slug: &str) -> String {
    format!("{SECTION}/{slug}")
}

fn file(name: &str) -> String {
    format!("https://medarbetare.su.se/download/18.1/1/{name}.pdf")
}

async fn read(db: &PgPool, url: &str, files: &[&str]) -> q::PageSummary {
    let files: Vec<FileInput<'_>> = files
        .iter()
        .map(|url| FileInput { url, title: "Fil" })
        .collect();
    q::record_page_success(
        db,
        &PageRead {
            url,
            title: "Sida",
            body: "Text",
            content_hash: "hash",
            updated_on: None,
            files: &files,
            next_check_at: chrono::Utc::now() + chrono::Duration::hours(24),
        },
    )
    .await
    .unwrap()
}

async fn selectable(db: &PgPool) -> Vec<String> {
    let mut urls: Vec<String> = q::pages_to_select(db)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.url)
        .collect();
    urls.sort();
    urls
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires an isolated PostgreSQL DATABASE_URL"]
async fn pages_wait_for_their_files_and_share_downloads(db: PgPool) {
    let (a, b, c) = (page("a"), page("b"), page("c"));
    let (x, y) = (file("x"), file("y"));
    let entries = [
        EntryInput {
            url: &a,
            title: "A",
            category: "Regler",
            is_file: false,
        },
        EntryInput {
            url: &b,
            title: "B",
            category: "Regler",
            is_file: false,
        },
    ];
    let summary = q::reconcile_section(&db, SECTION, &entries).await.unwrap();
    assert_eq!(summary.added, 2);

    read(&db, &a, &[&x, &y]).await;
    read(&db, &b, &[&x]).await;
    // Two pages link X; it is fetched once.
    let mut due = q::due_files(&db, 10).await.unwrap();
    due.sort();
    assert_eq!(due, [x.clone(), y.clone()]);
    assert!(selectable(&db).await.is_empty());

    q::record_file_success(&db, &x, "hx", 10, "[Page 1]\nX")
        .await
        .unwrap();
    assert_eq!(selectable(&db).await, std::slice::from_ref(&b));
    // A page linking X later takes it over downloaded.
    q::reconcile_section(
        &db,
        SECTION,
        &[
            entries[0],
            entries[1],
            EntryInput {
                url: &c,
                title: "C",
                category: "Stöddokument",
                is_file: false,
            },
        ],
    )
    .await
    .unwrap();
    read(&db, &c, &[&x]).await;
    assert_eq!(
        q::due_files(&db, 10).await.unwrap(),
        std::slice::from_ref(&y)
    );

    // A failing file stops holding its page back after its first attempt.
    q::record_file_failure(&db, &y, "http 404", 168)
        .await
        .unwrap();
    assert_eq!(selectable(&db).await, [a.clone(), b.clone(), c.clone()]);

    q::record_selection(&db, &a, true, std::slice::from_ref(&x))
        .await
        .unwrap();
    q::record_selection(&db, &b, false, std::slice::from_ref(&x))
        .await
        .unwrap();
    q::record_selection(&db, &c, false, &[]).await.unwrap();
    assert!(selectable(&db).await.is_empty());
    let docs: Vec<(String, bool)> = q::mirror_documents(&db)
        .await
        .unwrap()
        .into_iter()
        .map(|d| (d.url, d.is_page))
        .collect();
    assert_eq!(docs, [(a.clone(), true), (x.clone(), false)]);

    // A page that drops a file is decided again.
    let summary = read(&db, &a, &[&x]).await;
    assert_eq!(summary.files_delisted, 1);
    assert!(!summary.changed);
    assert_eq!(selectable(&db).await, std::slice::from_ref(&a));
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires an isolated PostgreSQL DATABASE_URL"]
async fn an_index_file_is_its_own_file_and_delisting_leaves_the_mirror(db: PgPool) {
    let list = file("lista");
    let entry = EntryInput {
        url: &list,
        title: "Lista över stödformer",
        category: "Stöddokument",
        is_file: true,
    };
    q::reconcile_section(&db, SECTION, &[entry]).await.unwrap();
    assert_eq!(
        q::due_files(&db, 10).await.unwrap(),
        std::slice::from_ref(&list)
    );
    // Never read as a page.
    assert!(q::due_pages(&db, 10).await.unwrap().is_empty());

    q::record_file_success(&db, &list, "hl", 10, "")
        .await
        .unwrap();
    assert_eq!(selectable(&db).await, std::slice::from_ref(&list));
    q::record_selection(&db, &list, false, std::slice::from_ref(&list))
        .await
        .unwrap();
    assert_eq!(q::mirror_documents(&db).await.unwrap().len(), 1);

    let summary = q::reconcile_section(&db, SECTION, &[]).await.unwrap();
    assert_eq!(summary.delisted, 1);
    assert!(q::mirror_documents(&db).await.unwrap().is_empty());
    let summary = q::reconcile_section(&db, SECTION, &[entry]).await.unwrap();
    assert_eq!(summary.relisted, 1);
    assert_eq!(q::mirror_documents(&db).await.unwrap().len(), 1);
}
