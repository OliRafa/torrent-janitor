//! Builders shared by the unit tests.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::api::{Arr, Qbit};
use crate::arr::{self, Kind, QualityProfile, QueueRecord};
use crate::config::Config;
use crate::qbit::{self, File, MainData, State, Torrent};
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
        amount_left: 1 << 30,
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

#[derive(Debug, Default)]
pub struct FakeQbit {
    pub torrents: RefCell<Vec<Torrent>>,
    pub free_space: Cell<Option<u64>>,
    pub files: RefCell<HashMap<String, Vec<File>>>,
    pub down: Cell<bool>,
    pub file_fetches: Cell<u32>,
    pub deleted: RefCell<Vec<String>>,
    pub stopped: RefCell<Vec<String>>,
    pub started: RefCell<Vec<String>>,
}

impl FakeQbit {
    fn each(&self, hashes: &[String], mut change: impl FnMut(&mut Torrent)) {
        for torrent in self.torrents.borrow_mut().iter_mut() {
            if hashes.contains(&torrent.hash) {
                change(torrent);
            }
        }
    }

    fn check(&self, path: &'static str) -> Result<(), qbit::Error> {
        if self.down.get() {
            return Err(qbit::Error::Status {
                path,
                status: reqwest::StatusCode::SERVICE_UNAVAILABLE,
            });
        }
        Ok(())
    }
}

impl Qbit for FakeQbit {
    async fn main_data(&self) -> Result<MainData, qbit::Error> {
        self.check("/api/v2/sync/maindata")?;
        Ok(MainData {
            torrents: self.torrents.borrow().clone(),
            free_space: self.free_space.get(),
        })
    }

    async fn files(&self, hash: &str) -> Result<Vec<File>, qbit::Error> {
        self.check("/api/v2/torrents/files")?;
        self.file_fetches.set(self.file_fetches.get() + 1);
        Ok(self.files.borrow().get(hash).cloned().unwrap_or_default())
    }

    async fn delete(&self, hashes: &[String]) -> Result<(), qbit::Error> {
        self.check("/api/v2/torrents/delete")?;
        self.deleted.borrow_mut().extend(hashes.iter().cloned());
        self.torrents
            .borrow_mut()
            .retain(|t| !hashes.contains(&t.hash));
        Ok(())
    }

    async fn stop(&self, hashes: &[String]) -> Result<(), qbit::Error> {
        self.check("/api/v2/torrents/stop")?;
        self.stopped.borrow_mut().extend(hashes.iter().cloned());
        self.each(hashes, |t| t.state = State::StoppedDL);
        Ok(())
    }

    async fn start(&self, hashes: &[String]) -> Result<(), qbit::Error> {
        self.check("/api/v2/torrents/start")?;
        self.started.borrow_mut().extend(hashes.iter().cloned());
        self.each(hashes, |t| t.state = State::Downloading);
        Ok(())
    }

    async fn add_tag(&self, hashes: &[String], tag: &str) -> Result<(), qbit::Error> {
        self.check("/api/v2/torrents/addTags")?;
        self.each(hashes, |t| {
            if !t.has_tag(tag) {
                t.tags = if t.tags.is_empty() {
                    tag.to_owned()
                } else {
                    format!("{}, {tag}", t.tags)
                };
            }
        });
        Ok(())
    }

    async fn remove_tag(&self, hashes: &[String], tag: &str) -> Result<(), qbit::Error> {
        self.check("/api/v2/torrents/removeTags")?;
        self.each(hashes, |t| {
            t.tags = t
                .tags
                .split(',')
                .map(str::trim)
                .filter(|x| *x != tag)
                .collect::<Vec<_>>()
                .join(", ");
        });
        Ok(())
    }
}

#[derive(Debug)]
pub struct FakeArr {
    pub kind: Kind,
    pub records: RefCell<Vec<QueueRecord>>,
    pub profiles: Vec<QualityProfile>,
    pub owners: HashMap<i64, i64>,
    pub down: Cell<bool>,
    pub profile_fetches: Cell<u32>,
    /// `(queue id, blocklist, skip_redownload)` per delete.
    pub deletes: RefCell<Vec<(i64, bool, bool)>>,
}

impl FakeArr {
    pub fn new(kind: Kind) -> Self {
        FakeArr {
            kind,
            records: RefCell::default(),
            profiles: Vec::new(),
            owners: HashMap::new(),
            down: Cell::new(false),
            profile_fetches: Cell::new(0),
            deletes: RefCell::default(),
        }
    }

    fn check(&self, path: &str) -> Result<(), arr::Error> {
        if self.down.get() {
            return Err(arr::Error::Status {
                kind: self.kind,
                path: path.to_owned(),
                status: reqwest::StatusCode::SERVICE_UNAVAILABLE,
            });
        }
        Ok(())
    }
}

impl Arr for FakeArr {
    fn kind(&self) -> Kind {
        self.kind
    }

    async fn queue(&self) -> Result<Vec<QueueRecord>, arr::Error> {
        self.check("/api/v3/queue")?;
        Ok(self.records.borrow().clone())
    }

    async fn quality_profiles(&self) -> Result<Vec<QualityProfile>, arr::Error> {
        self.check("/api/v3/qualityprofile")?;
        self.profile_fetches.set(self.profile_fetches.get() + 1);
        Ok(self.profiles.clone())
    }

    async fn profiles_by_owner(&self) -> Result<HashMap<i64, i64>, arr::Error> {
        self.check("/api/v3/series")?;
        Ok(self.owners.clone())
    }

    /// Like the real arr: one record id removes the whole download.
    async fn delete_queue_item(
        &self,
        id: i64,
        blocklist: bool,
        skip_redownload: bool,
    ) -> Result<(), arr::Error> {
        let path = format!("/api/v3/queue/{id}");
        self.check(&path)?;
        let mut records = self.records.borrow_mut();
        let Some(download) = records
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.download_id.clone())
        else {
            return Err(arr::Error::Status {
                kind: self.kind,
                path,
                status: reqwest::StatusCode::NOT_FOUND,
            });
        };
        records.retain(|r| r.download_id != download);
        self.deletes
            .borrow_mut()
            .push((id, blocklist, skip_redownload));
        Ok(())
    }
}
