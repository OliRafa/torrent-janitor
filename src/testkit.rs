//! Builders shared by the unit tests.

use std::collections::HashMap;

use crate::arr::QueueRecord;
use crate::config::Config;
use crate::qbit::{File, State, Torrent};
use crate::snapshot::{Download, Snapshot};

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

pub fn file(name: &str, priority: u8) -> File {
    File {
        index: 0,
        name: name.to_owned(),
        size: 1 << 20,
        priority,
    }
}

pub fn download(hash: &str, tracked_state: &str, messages: &[&str]) -> Download {
    Download {
        arr: 0,
        hash: hash.to_owned(),
        queue_id: 1,
        title: format!("Example.{hash}"),
        items: vec![101],
        tracked_state: Some(tracked_state.to_owned()),
        messages: messages.iter().map(|m| (*m).to_owned()).collect(),
        quality_rank: None,
        custom_format_score: 0,
    }
}

/// A cycle every removal rule has something to say about.
pub fn troubled_snapshot() -> Snapshot {
    Snapshot {
        torrents: vec![
            torrent("fake", State::StalledDL, "tv-sonarr"),
            torrent("done", State::StalledUP, "tv-sonarr"),
        ],
        files: HashMap::from([("fake".to_owned(), vec![file("Example.S01E01.exe", 0)])]),
        downloads: vec![download(
            "done",
            "importBlocked",
            &["Not an upgrade for existing episode file(s)."],
        )],
        ..Snapshot::default()
    }
}
