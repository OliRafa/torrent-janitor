//! A release that brings no media but does bring an executable is a fake,
//! like the lone `.exe` named after the episode it claimed to be. Files
//! qBit's `ExcludedFileNames` skipped still show in the file list, so they
//! are found here too. Deselected files alone prove nothing: genuine
//! releases end up with every file deselected as well.

use globset::GlobSet;

use super::{Reason, Removal};
use crate::snapshot::Snapshot;

pub(super) fn evaluate(
    snapshot: &Snapshot,
    media: &GlobSet,
    executables: &GlobSet,
) -> Vec<Removal> {
    snapshot
        .torrents
        .iter()
        .filter_map(|torrent| {
            let files = snapshot
                .files
                .get(&torrent.hash)
                .filter(|f| !f.is_empty())?;
            if files
                .iter()
                .any(|f| f.priority > 0 && media.is_match(basename(&f.name)))
            {
                return None;
            }
            let executable = files
                .iter()
                .find(|f| f.name.split('/').any(|part| executables.is_match(part)))?;
            Some(Removal {
                hash: torrent.hash.clone(),
                reason: Reason::Fake,
                blocklist: true,
                research: true,
                detail: format!("no media file, executable {}", executable.name),
            })
        })
        .collect()
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
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
            torrents: vec![torrent("a", State::StalledDL, "tv-sonarr")],
            files: HashMap::from([("a".to_owned(), files)]),
            ..Snapshot::default()
        };
        let rules = RuleSet::new(&Default::default()).unwrap();
        evaluate(&snapshot, &rules.media, &rules.executables)
    }

    #[test]
    fn a_lone_executable_is_fake() {
        let found = removals(vec![file("Example.S01E01.1080p.exe", 0)]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].reason, Reason::Fake);
        assert!(found[0].blocklist && found[0].research);
        assert!(found[0].detail.contains("Example.S01E01.1080p.exe"));
    }

    #[test]
    fn a_release_with_media_and_an_executable_is_not_fake() {
        let found = removals(vec![
            file("Example/Example.S01E01.mkv", 1),
            file("Example/Codec.Installer.exe", 0),
        ]);
        assert!(found.is_empty());
    }

    #[test]
    fn media_the_torrent_will_not_download_does_not_count() {
        let found = removals(vec![
            file("Example/Example.S01E01.mkv", 0),
            file("Example/Player.exe", 1),
        ]);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn a_genuine_release_with_every_file_deselected_is_not_fake() {
        assert!(removals(vec![file("Example (2006).avi", 0)]).is_empty());
        assert!(
            removals(vec![
                file("Example/readme.nfo", 0),
                file("Example/x.dat", 0)
            ])
            .is_empty()
        );
    }

    #[test]
    fn a_release_without_media_or_executables_is_left_to_the_arr() {
        assert!(
            removals(vec![
                file("Example/Example.rar", 1),
                file("Example/Example.nfo", 1)
            ])
            .is_empty()
        );
    }

    #[test]
    fn patterns_match_any_case_and_any_folder() {
        assert_eq!(removals(vec![file("Example.S01E01.EXE", 1)]).len(), 1);
        assert_eq!(
            removals(vec![file("Setup.app/Contents/Info.plist", 1)]).len(),
            1,
            "a macOS bundle is an executable folder"
        );
    }

    #[test]
    fn a_torrent_without_metadata_is_not_judged() {
        assert!(removals(vec![]).is_empty());
        let snapshot = Snapshot {
            torrents: vec![torrent("a", State::MetaDL, "tv-sonarr")],
            ..Snapshot::default()
        };
        let rules = RuleSet::new(&Default::default()).unwrap();
        assert!(evaluate(&snapshot, &rules.media, &rules.executables).is_empty());
    }
}
