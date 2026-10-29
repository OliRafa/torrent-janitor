//! A whole disc instead of video files: a Blu-ray `BDMV` tree, a DVD
//! `VIDEO_TS` folder or a disc image. Release titles often hide it, so this
//! looks at the files.

use globset::GlobSet;

use super::{Reason, Removal};
use crate::snapshot::Snapshot;

pub(super) fn evaluate(snapshot: &Snapshot, patterns: &GlobSet) -> Vec<Removal> {
    snapshot
        .torrents
        .iter()
        .filter_map(|torrent| {
            let disc = snapshot
                .files
                .get(&torrent.hash)?
                .iter()
                .find(|f| f.name.split('/').any(|part| patterns.is_match(part)))?;
            Some(Removal {
                hash: torrent.hash.clone(),
                reason: Reason::Disc,
                blocklist: true,
                research: true,
                detail: format!("disc layout, {}", disc.name),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::qbit::{File, State};
    use crate::rules::RuleSet;
    use crate::testkit::{file, torrent};

    fn removals(files: Vec<File>) -> Vec<Removal> {
        let snapshot = Snapshot {
            torrents: vec![torrent("a", State::StalledDL, "movies-radarr")],
            files: HashMap::from([("a".to_owned(), files)]),
            ..Snapshot::default()
        };
        let rules = RuleSet::new(&Default::default()).unwrap();
        evaluate(&snapshot, &rules.disc)
    }

    #[test]
    fn a_blu_ray_tree_is_a_disc_even_when_deselected() {
        let found = removals(vec![
            file("Example/BDMV/STREAM/00000.m2ts", 0),
            file("Example/BDMV/index.bdmv", 0),
        ]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].reason, Reason::Disc);
        assert!(found[0].blocklist && found[0].research);
        assert!(found[0].detail.contains("Example/BDMV/STREAM/00000.m2ts"));
    }

    #[test]
    fn dvd_folders_and_disc_images_are_discs() {
        assert_eq!(
            removals(vec![file("Example/VIDEO_TS/VTS_01_1.VOB", 1)]).len(),
            1
        );
        assert_eq!(removals(vec![file("Example (2006).ISO", 1)]).len(), 1);
        assert_eq!(removals(vec![file("Example/Disc1.img", 1)]).len(), 1);
    }

    #[test]
    fn a_remux_is_not_a_disc() {
        assert!(
            removals(vec![
                file("Example/Example.2160p.Remux.mkv", 1),
                file("Example/Example.nfo", 1),
            ])
            .is_empty()
        );
    }

    #[test]
    fn a_folder_only_named_like_a_disc_does_not_count() {
        assert!(removals(vec![file("Example.BDMV.Rip/Example.mkv", 1)]).is_empty());
    }

    #[test]
    fn a_torrent_without_metadata_is_not_judged() {
        assert!(removals(vec![]).is_empty());
    }
}
