use std::error::Error;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use tokio::signal::unix::{SignalKind, signal};
use tokio::time::MissedTickBehavior;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use torrent_janitor::config::Config;
use torrent_janitor::http::{self, Health};
use torrent_janitor::janitor::Janitor;
use torrent_janitor::metrics::Metrics;
use torrent_janitor::rules::RuleSet;
use torrent_janitor::{arr, error_chain, now, qbit};

const DEFAULT_CONFIG: &str = "/config/config.toml";

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .json()
        .flatten_event(true)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let path = std::env::var("JANITOR_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG.to_owned());
    match run(&path).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!(error = %error_chain(e.as_ref()), "cannot start");
            ExitCode::FAILURE
        }
    }
}

async fn run(path: &str) -> Result<(), Box<dyn Error>> {
    let config = Config::load(path)?;
    let rules = RuleSet::new(&config.rules)?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let qbit = qbit::Client::new(&config.qbit.url, http.clone());
    let version = qbit.check_version().await?;
    let mut arrs = Vec::with_capacity(config.arr.len());
    for a in &config.arr {
        arrs.push(arr::Client::new(a.kind, &a.url, a.api_key()?, http.clone()));
    }

    let (metrics, registry) = Metrics::new(config.dry_run);
    let interval = config.interval;
    let health = Arc::new(Health::new(now(), 5 * secs(interval)));
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let server = axum::serve(listener, http::router(registry, health.clone()));
    tokio::spawn(server.into_future());

    info!(
        webapi = %version,
        dry_run = config.dry_run,
        interval = %humantime_serde::re::humantime::format_duration(interval),
        arrs = config.arr.len(),
        "started"
    );
    let mut janitor = Janitor::new(config, rules, qbit, arrs, metrics);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut terminate = signal(SignalKind::terminate())?;
    loop {
        // A cycle runs to the end before a signal is looked at, so shutdown
        // never cuts a removal in half.
        tokio::select! {
            _ = ticker.tick() => {
                let now = now();
                janitor.tick(now).await;
                health.ticked(now);
            }
            _ = terminate.recv() => break,
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    info!("stopping");
    Ok(())
}

fn secs(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}
