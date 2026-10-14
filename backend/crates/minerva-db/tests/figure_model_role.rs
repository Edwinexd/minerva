use minerva_db::queries::chat_models as models;

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires an isolated PostgreSQL DATABASE_URL"]
async fn figure_role_is_independent_unique_and_requires_an_enabled_model(db: sqlx::PgPool) {
    assert!(models::current_figure_description_default(&db)
        .await
        .unwrap()
        .is_none());
    sqlx::query("INSERT INTO chat_models (model,provider,display_name,enabled,input_usd_per_mtok,output_usd_per_mtok) VALUES ('vision-a','openai','Vision A',TRUE,0.1,0.5),('vision-b','openai','Vision B',TRUE,0.1,0.5),('disabled-vision','openai','Disabled',FALSE,NULL,NULL)")
        .execute(&db).await.unwrap();
    let chat_default = models::current_default(&db).await.unwrap();
    let utility_default = models::current_utility_default(&db).await.unwrap();
    models::set_figure_description_default(&db, "vision-a")
        .await
        .unwrap();
    let row = models::set_figure_description_default(&db, "vision-b")
        .await
        .unwrap();
    assert!(row.is_figure_description_default);
    assert!(
        !models::find(&db, "vision-a")
            .await
            .unwrap()
            .unwrap()
            .is_figure_description_default
    );
    assert!(matches!(
        models::set_figure_description_default(&db, "disabled-vision").await,
        Err(models::SetDefaultError::Disabled)
    ));
    assert_eq!(
        models::current_figure_description_default(&db)
            .await
            .unwrap()
            .as_deref(),
        Some("vision-b")
    );
    assert_eq!(models::current_default(&db).await.unwrap(), chat_default);
    assert_eq!(
        models::current_utility_default(&db).await.unwrap(),
        utility_default
    );
    models::clear_figure_description_default(&db).await.unwrap();
    assert!(models::current_figure_description_default(&db)
        .await
        .unwrap()
        .is_none());
}
