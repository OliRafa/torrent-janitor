use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Sonarr,
    Radarr,
}

/// One queue record. Sonarr lists a season pack once per episode, all with
/// the same `download_id`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueRecord {
    pub id: i64,
    pub download_id: Option<String>,
    pub title: String,
    pub status: Option<String>,
    pub tracked_download_status: Option<String>,
    pub tracked_download_state: Option<String>,
    #[serde(default)]
    pub status_messages: Vec<StatusMessage>,
    pub error_message: Option<String>,
    pub episode_id: Option<i64>,
    pub series_id: Option<i64>,
    pub movie_id: Option<i64>,
    pub quality: Option<QualityModel>,
    #[serde(default)]
    pub custom_format_score: i64,
    pub protocol: Option<String>,
}

impl QueueRecord {
    /// The episode or movie this record is for.
    pub fn media_item(&self) -> Option<i64> {
        self.episode_id.or(self.movie_id)
    }

    /// The series or movie whose quality profile ranks this record.
    pub fn profile_owner(&self) -> Option<i64> {
        self.series_id.or(self.movie_id)
    }

    pub fn quality_id(&self) -> Option<i64> {
        self.quality.as_ref().map(|q| q.quality.id)
    }

    pub fn messages(&self) -> impl Iterator<Item = &str> {
        self.status_messages
            .iter()
            .flat_map(|m| m.messages.iter().map(String::as_str))
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct StatusMessage {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub messages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct QualityModel {
    pub quality: Quality,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Quality {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct QualityProfile {
    pub id: i64,
    pub items: Vec<ProfileItem>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ProfileItem {
    pub quality: Option<Quality>,
    #[serde(default)]
    pub items: Vec<ProfileItem>,
}

impl QualityProfile {
    /// Position of a quality in the profile; higher is better. The arrs list
    /// items worst to best, and every member of a group shares the group's
    /// position.
    pub fn rank(&self, quality_id: i64) -> Option<usize> {
        self.items.iter().position(|item| {
            item.quality.as_ref().is_some_and(|q| q.id == quality_id)
                || item
                    .items
                    .iter()
                    .any(|i| i.quality.as_ref().is_some_and(|q| q.id == quality_id))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILES: &str = include_str!("../../tests/fixtures/arr/sonarr-qualityprofiles.json");

    #[test]
    fn later_profile_items_rank_higher_and_groups_share_a_rank() {
        let profile: QualityProfile = serde_json::from_str::<Vec<QualityProfile>>(PROFILES)
            .unwrap()
            .remove(0);
        let hdtv_720 = profile.rank(4).unwrap();
        let webrip_1080 = profile.rank(15).unwrap();
        let webdl_1080 = profile.rank(3).unwrap();
        assert!(webrip_1080 > hdtv_720);
        assert_eq!(webrip_1080, webdl_1080, "both sit in the WEB 1080p group");
        assert_eq!(profile.rank(9_999), None);
    }
}
