use std::net::SocketAddr;
use std::time::Duration;

use secret_santa::config::Config;
use secret_santa::{purge_expired_sessions, router, setup};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "secret_santa=info,tower_http=info".into()),
        )
        .init();

    let cfg = Config::from_env();
    let bind = cfg.bind.clone();
    let state = setup(cfg).await?;

    let db = state.db.clone();
    tokio::spawn(async move {
        loop {
            purge_expired_sessions(&db).await;
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });

    let app = router(state);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("Secret Santa démarré sur http://{bind}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
