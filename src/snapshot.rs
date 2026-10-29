//! One consistent view of a cycle: the managed torrents joined to the arr
//! queue records that own them. Rules read only this.

use std::collections::{BTreeSet, HashMap};

use crate::arr::{QualityProfile, QueueRecord};
use crate::config::Config;
use crate::qbit::{File, Torrent};

/// One arr's side of a cycle.
#[derive(Debug, Clone, Default)]
pub struct ArrView {
    pub records: Vec<QueueRecord>,
    pub profiles: Vec<QualityProfile>,
    /// Quality profile id per series or movie.
    pub owners: HashMap<i64, i64>,
}

/// A torrent as its arr sees it: one entry per download, however many
/// episodes it covers.
#[derive(Debug, Clone, PartialEq)]
pub struct Download {
    pub arr: usize,
    pub hash: String,
    /// Any of its queue records; deleting one removes the whole download.
    pub queue_id: i64,
    pub title: String,
    pub items: Vec<i64>,
    pub tracked_state: Option<String>,
    pub messages: Vec<String>,
    pub quality_rank: Option<usize>,
    pub custom_format_score: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub now: i64,
    pub free_space: Option<u64>,
    pub torrents: Vec<Torrent>,
    pub files: HashMap<String, Vec<File>>,
    pub downloads: Vec<Download>,
    /// Torrents in an arr's category that its queue does not list.
    pub untracked: Vec<String>,
}

impl Snapshot {
    pub fn build(
        config: &Config,
        now: i64,
        torrents: Vec<Torrent>,
        free_space: Option<u64>,
        mut files: HashMap<String, Vec<File>>,
        arrs: &[ArrView],
    ) -> Snapshot {
        let torrents: Vec<Torrent> = torrents
            .into_iter()
            .filter(|t| config.manages(&t.category))
            .collect();
        files.retain(|hash, _| torrents.iter().any(|t| &t.hash == hash));

        let mut downloads = Vec::new();
        for (arr, view) in arrs.iter().enumerate() {
            let Some(category) = config.arr.get(arr).map(|a| a.category.as_str()) else {
                continue;
            };
            let mut grouped: HashMap<String, Vec<&QueueRecord>> = HashMap::new();
            for record in &view.records {
                let Some(id) = &record.download_id else {
                    continue;
                };
                let hash = id.to_ascii_lowercase();
                if torrents
                    .iter()
                    .any(|t| t.hash == hash && t.category == category)
                {
                    grouped.entry(hash).or_default().push(record);
                }
            }
            for (hash, records) in grouped {
                downloads.push(download(arr, hash, &records, view));
            }
        }
        downloads.sort_by(|a, b| (a.arr, &a.hash).cmp(&(b.arr, &b.hash)));
        let untracked = torrents
            .iter()
            .filter(|t| config.owner(&t.category).is_some())
            .filter(|t| !downloads.iter().any(|d| d.hash == t.hash))
            .map(|t| t.hash.clone())
            .collect();

        Snapshot {
            now,
            free_space,
            torrents,
            files,
            downloads,
            untracked,
        }
    }

    pub fn torrent(&self, hash: &str) -> Option<&Torrent> {
        self.torrents.iter().find(|t| t.hash == hash)
    }

    pub fn download(&self, hash: &str) -> Option<&Download> {
        self.downloads.iter().find(|d| d.hash == hash)
    }
}

fn download(arr: usize, hash: String, records: &[&QueueRecord], view: &ArrView) -> Download {
    let first = records[0];
    let items: BTreeSet<i64> = records.iter().filter_map(|r| r.media_item()).collect();
    let messages: BTreeSet<String> = records
        .iter()
        .flat_map(|r| r.messages())
        .map(str::to_owned)
        .collect();
    let profile = first
        .profile_owner()
        .and_then(|owner| view.owners.get(&owner))
        .and_then(|id| view.profiles.iter().find(|p| p.id == *id));
    Download {
        arr,
        hash,
        queue_id: records.iter().map(|r| r.id).min().unwrap_or(first.id),
        title: first.title.clone(),
        items: items.into_iter().collect(),
        tracked_state: first.tracked_download_state.clone(),
        messages: messages.into_iter().collect(),
        quality_rank: profile.zip(first.quality_id()).and_then(|(p, q)| p.rank(q)),
        custom_format_score: first.custom_format_score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qbit::State;
    use crate::testkit::{config, record, torrent};

    #[test]
    fn a_season_pack_becomes_one_download_over_its_episodes() {
        let config = config();
        let view = ArrView {
            records: vec![
                record(7, "AB", 102),
                record(5, "AB", 101),
                record(6, "AB", 103),
            ],
            ..ArrView::default()
        };
        let snapshot = Snapshot::build(
            &config,
            0,
            vec![torrent("ab", State::StalledDL, "tv-sonarr")],
            None,
            HashMap::new(),
            &[view],
        );
        assert_eq!(snapshot.downloads.len(), 1);
        let download = &snapshot.downloads[0];
        assert_eq!(download.items, [101, 102, 103]);
        assert_eq!(download.queue_id, 5);
        assert_eq!(
            download.hash, "ab",
            "arr ids are upper case, qBittorrent's are not"
        );
    }

    #[test]
    fn only_managed_categories_are_seen() {
        let snapshot = Snapshot::build(
            &config(),
            0,
            vec![
                torrent("a", State::StalledDL, "tv-sonarr"),
                torrent("b", State::StalledDL, "games"),
            ],
            None,
            HashMap::from([("b".to_owned(), vec![])]),
            &[ArrView::default()],
        );
        assert_eq!(snapshot.torrents.len(), 1);
        assert!(snapshot.files.is_empty());
    }

    #[test]
    fn an_arr_cannot_claim_a_torrent_in_another_category() {
        let view = ArrView {
            records: vec![record(1, "AB", 101)],
            ..ArrView::default()
        };
        let snapshot = Snapshot::build(
            &config(),
            0,
            vec![torrent("ab", State::StalledDL, "movies-radarr")],
            None,
            HashMap::new(),
            &[view],
        );
        assert!(snapshot.downloads.is_empty());
    }

    #[test]
    fn records_without_a_torrent_are_ignored() {
        let view = ArrView {
            records: vec![record(1, "CD", 101)],
            ..ArrView::default()
        };
        let snapshot = Snapshot::build(&config(), 0, vec![], None, HashMap::new(), &[view]);
        assert!(snapshot.downloads.is_empty());
    }

    #[test]
    fn torrents_an_arr_does_not_list_are_untracked() {
        let view = ArrView {
            records: vec![record(1, "AB", 101)],
            ..ArrView::default()
        };
        let snapshot = Snapshot::build(
            &config(),
            0,
            vec![
                torrent("ab", State::StalledDL, "tv-sonarr"),
                torrent("cd", State::StalledDL, "tv-sonarr"),
                torrent("ef", State::StalledDL, "comics"),
            ],
            None,
            HashMap::new(),
            &[view],
        );
        assert_eq!(
            snapshot.untracked,
            ["cd"],
            "qBit-only categories have no queue"
        );
    }

    #[test]
    fn quality_is_ranked_through_the_owners_profile() {
        let profiles: Vec<QualityProfile> = serde_json::from_str(include_str!(
            "../tests/fixtures/arr/sonarr-qualityprofiles.json"
        ))
        .unwrap();
        let mut webrip = record(1, "AB", 101);
        webrip.series_id = Some(9);
        webrip.quality = Some(crate::arr::QualityModel {
            quality: crate::arr::Quality {
                id: 15,
                name: "WEBRip-1080p".into(),
            },
        });
        let view = ArrView {
            records: vec![webrip],
            profiles,
            owners: HashMap::from([(9, 14)]),
        };
        let snapshot = Snapshot::build(
            &config(),
            0,
            vec![torrent("ab", State::StalledDL, "tv-sonarr")],
            None,
            HashMap::new(),
            &[view],
        );
        assert!(snapshot.downloads[0].quality_rank.is_some());
    }
}
