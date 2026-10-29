//! `/healthz` and `/metrics`. Health means the loop is still ticking, not
//! that qBittorrent and the arrs answer: restarting the janitor cannot fix
//! them, and the failed cycles already show up in the metrics.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use prometheus_client::encoding::text::encode;
use prometheus_client::registry::Registry;

#[derive(Debug)]
pub struct Health {
    last_tick: AtomicI64,
    stale_after: i64,
}

impl Health {
    pub fn new(now: i64, stale_after: i64) -> Self {
        Health {
            last_tick: AtomicI64::new(now),
            stale_after,
        }
    }

    pub fn ticked(&self, now: i64) {
        self.last_tick.store(now, Ordering::Relaxed);
    }

    fn healthy(&self, now: i64) -> bool {
        now - self.last_tick.load(Ordering::Relaxed) <= self.stale_after
    }
}

#[derive(Clone)]
struct Shared {
    registry: Arc<Registry>,
    health: Arc<Health>,
}

pub fn router(registry: Registry, health: Arc<Health>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .with_state(Shared {
            registry: Arc::new(registry),
            health,
        })
}

async fn healthz(State(shared): State<Shared>) -> StatusCode {
    if shared.health.healthy(crate::now()) {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

async fn metrics(State(shared): State<Shared>) -> impl IntoResponse {
    let mut body = String::new();
    match encode(&mut body, &shared.registry) {
        Ok(()) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                "application/openmetrics-text; version=1.0.0; charset=utf-8",
            )],
            body,
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn serve(health: Arc<Health>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (_, registry) = crate::metrics::Metrics::new(true);
        tokio::spawn(axum::serve(listener, router(registry, health)).into_future());
        format!("http://{address}")
    }

    #[tokio::test]
    async fn metrics_are_served_in_openmetrics_text() {
        let base = serve(Arc::new(Health::new(crate::now(), 300))).await;
        let response = reqwest::get(format!("{base}/metrics")).await.unwrap();
        assert_eq!(response.status(), 200);
        assert!(
            response
                .text()
                .await
                .unwrap()
                .contains("torrent_janitor_dry_run 1")
        );
    }

    #[tokio::test]
    async fn health_fails_once_the_loop_stops_ticking() {
        let health = Arc::new(Health::new(crate::now(), 300));
        let base = serve(health.clone()).await;
        let status = |base: String| async move {
            reqwest::get(format!("{base}/healthz"))
                .await
                .unwrap()
                .status()
        };
        assert_eq!(status(base.clone()).await, 200);
        health.ticked(crate::now() - 301);
        assert_eq!(status(base).await, 503);
    }
}
