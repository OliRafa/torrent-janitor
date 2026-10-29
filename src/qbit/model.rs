use serde::Deserialize;

/// Torrent states as named by WebAPI 2.11+ (qBittorrent 5). The 4.x names
/// `pausedUP`/`pausedDL` land in `Unknown`, which no rule acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Error,
    MissingFiles,
    Uploading,
    StoppedUP,
    QueuedUP,
    StalledUP,
    CheckingUP,
    ForcedUP,
    Allocating,
    Downloading,
    MetaDL,
    ForcedMetaDL,
    StoppedDL,
    QueuedDL,
    StalledDL,
    CheckingDL,
    ForcedDL,
    CheckingResumeData,
    Moving,
    #[serde(other)]
    Unknown,
}

impl State {
    pub fn is_metadata(self) -> bool {
        matches!(self, State::MetaDL | State::ForcedMetaDL)
    }

    /// Started, wanting data, and getting none. Queued, stopped and checking
    /// torrents are not here: time spent in those states is never a stall.
    pub fn is_stalled(self) -> bool {
        self.is_metadata() || self == State::StalledDL
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Torrent {
    /// `torrents/info` carries the hash; `sync/maindata` only uses it as the
    /// map key, and the client copies it in.
    #[serde(default)]
    pub hash: String,
    pub name: String,
    pub state: State,
    pub category: String,
    #[serde(default)]
    pub tags: String,
    pub added_on: i64,
    pub progress: f64,
    pub downloaded: u64,
    pub size: i64,
}

impl Torrent {
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.split(',').any(|t| t.trim() == tag)
    }

    pub fn is_complete(&self) -> bool {
        self.progress >= 1.0
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct File {
    pub index: u32,
    pub name: String,
    pub size: u64,
    pub priority: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MainData {
    pub torrents: Vec<Torrent>,
    /// Free bytes under the default save path, as qBittorrent itself sees
    /// them. In the cluster the qgroup-fence shim makes this quota-aware.
    pub free_space: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_states_do_not_fail_decoding() {
        let state: State = serde_json::from_str("\"pausedDL\"").unwrap();
        assert_eq!(state, State::Unknown);
    }

    #[test]
    fn queued_and_stopped_torrents_are_not_stalled() {
        for state in [
            State::QueuedDL,
            State::StoppedDL,
            State::CheckingDL,
            State::Downloading,
        ] {
            assert!(!state.is_stalled(), "{state:?}");
        }
        for state in [State::MetaDL, State::ForcedMetaDL, State::StalledDL] {
            assert!(state.is_stalled(), "{state:?}");
        }
    }

    #[test]
    fn tags_are_matched_whole() {
        let torrent = Torrent {
            hash: String::new(),
            name: String::new(),
            state: State::Downloading,
            category: String::new(),
            tags: "janitor:space, other".into(),
            added_on: 0,
            progress: 0.0,
            downloaded: 0,
            size: 0,
        };
        assert!(torrent.has_tag("janitor:space"));
        assert!(torrent.has_tag("other"));
        assert!(!torrent.has_tag("janitor"));
    }
}
