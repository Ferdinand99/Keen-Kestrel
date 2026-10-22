//! Runs the web server until Ctrl-C or SIGTERM, then lets requests in
//! flight finish.

use std::net::SocketAddr;

use crate::config::Config;

/// Starts Kestrel with `config` and serves until it is told to stop. With
/// `KK_INGEST_PORT`, a second listener serves ingest alone. Events accepted
/// before the stop are written before it returns.
pub async fn run(config: Config) -> Result<(), crate::StartError> {
    let kestrel = crate::start_with(
        &config.data_dir,
        config.public_url.as_deref(),
        config.ingest_settings(),
    )
    .await?;
    // Notifications and other work outside a request use the site language.
    vole_kit::i18n::site_language(&kestrel.core).await;

    let addr = config.bind_addr();
    let listener = bind(addr).await?;
    let ingest_listener = match config.ingest_addr() {
        Some(addr) => Some(bind(addr).await?),
        None => None,
    };
    tracing::info!(
        version = crate::VERSION,
        data_dir = %config.data_dir.display(),
        "Keen Kestrel listening on http://{addr}"
    );

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let stopped = move || {
        let mut rx = stop_rx.clone();
        async move {
            let _ = rx.wait_for(|stop| *stop).await;
        }
    };
    // Connect info lets the rate limiters key on the peer address.
    let main = axum::serve(
        listener,
        crate::router(kestrel.clone()).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(stopped());
    let ingest = ingest_listener.map(|listener| {
        tracing::info!(
            "ingest also listening on http://{}",
            listener
                .local_addr()
                .map(|a| a.to_string())
                .unwrap_or_default()
        );
        axum::serve(
            listener,
            crate::ingest_router(kestrel.clone())
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(stopped())
    });
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = stop_tx.send(true);
    });
    match ingest {
        Some(ingest) => {
            let (a, b) = tokio::join!(main, ingest);
            a?;
            b?;
        }
        None => main.await?,
    }
    kestrel.ingest.flush().await;
    tracing::info!("stopped");
    Ok(())
}

async fn bind(addr: SocketAddr) -> Result<tokio::net::TcpListener, crate::StartError> {
    Ok(tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("can't listen on {addr}: {e}"))?)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::warn!(%err, "can't listen for Ctrl-C");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => {
                tracing::warn!(%err, "can't listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutting down");
}
