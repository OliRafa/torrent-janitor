//! Builders shared by the unit tests.

use crate::arr::QueueRecord;
use crate::config::Config;
use crate::qbit::{State, Torrent};

pub fn config() -> Config {
    Config::parse(
        r#"
        [qbit]
        url = "http://qbittorrent:8080"
        categories = ["comics"]

        [[arr]]
        kind = "sonarr"
        url = "http://sonarr:8989/sonarr"
        api_key_env = "SONARR_API_KEY"
        category = "tv-sonarr"

        [[arr]]
        kind = "radarr"
        url = "http://radarr:7878/radarr"
        api_key_env = "RADARR_API_KEY"
        category = "movies-radarr"
        "#,
    )
    .unwrap()
}

pub fn torrent(hash: &str, state: State, category: &str) -> Torrent {
    Torrent {
        hash: hash.to_owned(),
        name: format!("Example.{hash}"),
        state,
        category: category.to_owned(),
        tags: String::new(),
        added_on: 0,
        progress: 0.0,
        downloaded: 0,
        size: 1 << 30,
    }
}

pub fn record(id: i64, download_id: &str, episode: i64) -> QueueRecord {
    QueueRecord {
        id,
        download_id: Some(download_id.to_owned()),
        title: format!("Example.{download_id}"),
        status: Some("downloading".into()),
        tracked_download_status: Some("ok".into()),
        tracked_download_state: Some("downloading".into()),
        status_messages: vec![],
        error_message: None,
        episode_id: Some(episode),
        series_id: Some(1),
        movie_id: None,
        quality: None,
        custom_format_score: 0,
        protocol: Some("torrent".into()),
    }
}
