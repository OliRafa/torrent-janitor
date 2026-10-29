//! The slice of the Sonarr/Radarr v3 API the janitor uses. Both share the
//! queue endpoints; they differ in which media list holds the profile ids.

mod model;

use std::collections::HashMap;

use serde::Deserialize;
use serde::de::DeserializeOwned;

pub use model::{
    Kind, ProfileItem, Quality, QualityModel, QualityProfile, QueueRecord, StatusMessage,
};

const PAGE_SIZE: usize = 500;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{kind:?} request to {path} failed")]
    Request {
        kind: Kind,
        path: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("{kind:?} answered {status} to {path}")]
    Status {
        kind: Kind,
        path: String,
        status: reqwest::StatusCode,
    },
}

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    api_key: String,
    kind: Kind,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    total_records: usize,
    records: Vec<QueueRecord>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Media {
    id: i64,
    quality_profile_id: i64,
}

impl Client {
    /// `base` includes the arr's UrlBase (`http://sonarr:8989/sonarr`):
    /// redirects are not followed.
    pub fn new(kind: Kind, base: &str, api_key: String, http: reqwest::Client) -> Self {
        Client {
            http,
            base: base.trim_end_matches('/').to_owned(),
            api_key,
            kind,
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Every queue record, all pages, unknown items included.
    pub async fn queue(&self) -> Result<Vec<QueueRecord>, Error> {
        let unknown = match self.kind {
            Kind::Sonarr => "includeUnknownSeriesItems",
            Kind::Radarr => "includeUnknownMovieItems",
        };
        let page_size = PAGE_SIZE.to_string();
        let mut records = Vec::new();
        for page in 1.. {
            let page = page.to_string();
            let query = [
                ("page", page.as_str()),
                ("pageSize", page_size.as_str()),
                (unknown, "true"),
            ];
            let batch: Page = self.get_json("/api/v3/queue", &query).await?;
            let done = batch.records.is_empty()
                || records.len() + batch.records.len() >= batch.total_records;
            records.extend(batch.records);
            if done {
                break;
            }
        }
        Ok(records)
    }

    /// Deleting one record removes the whole download, every episode of a
    /// season pack included.
    pub async fn delete_queue_item(
        &self,
        id: i64,
        blocklist: bool,
        skip_redownload: bool,
    ) -> Result<(), Error> {
        let path = format!("/api/v3/queue/{id}");
        let query = [
            ("removeFromClient", "true"),
            ("blocklist", bool_str(blocklist)),
            ("skipRedownload", bool_str(skip_redownload)),
        ];
        let request = self.http.delete(self.url(&path)).query(&query);
        self.send(request, &path).await.map(drop)
    }

    pub async fn quality_profiles(&self) -> Result<Vec<QualityProfile>, Error> {
        self.get_json("/api/v3/qualityprofile", &[]).await
    }

    /// Quality profile id per series (Sonarr) or movie (Radarr).
    pub async fn profiles_by_owner(&self) -> Result<HashMap<i64, i64>, Error> {
        let path = match self.kind {
            Kind::Sonarr => "/api/v3/series",
            Kind::Radarr => "/api/v3/movie",
        };
        let media: Vec<Media> = self.get_json(path, &[]).await?;
        Ok(media
            .into_iter()
            .map(|m| (m.id, m.quality_profile_id))
            .collect())
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, Error> {
        let request = self.http.get(self.url(path)).query(query);
        self.send(request, path)
            .await?
            .json()
            .await
            .map_err(|source| Error::Request {
                kind: self.kind,
                path: path.to_owned(),
                source,
            })
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        path: &str,
    ) -> Result<reqwest::Response, Error> {
        let response = request
            .header("X-Api-Key", &self.api_key)
            .send()
            .await
            .map_err(|source| Error::Request {
                kind: self.kind,
                path: path.to_owned(),
                source,
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Status {
                kind: self.kind,
                path: path.to_owned(),
                status,
            });
        }
        Ok(response)
    }
}

fn bool_str(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SONARR_QUEUE: &str = include_str!("../../tests/fixtures/arr/sonarr-queue.json");
    const RADARR_QUEUE: &str = include_str!("../../tests/fixtures/arr/radarr-queue.json");

    fn client(kind: Kind, server: &MockServer) -> Client {
        Client::new(
            kind,
            &format!("{}/sonarr/", server.uri()),
            "secret".into(),
            reqwest::Client::new(),
        )
    }

    #[tokio::test]
    async fn reads_a_season_pack_as_one_download_over_several_episodes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sonarr/api/v3/queue"))
            .and(header("X-Api-Key", "secret"))
            .and(query_param("includeUnknownSeriesItems", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_string(SONARR_QUEUE))
            .mount(&server)
            .await;
        let records = client(Kind::Sonarr, &server).queue().await.unwrap();

        assert_eq!(records.len(), 5);
        let pack: Vec<&QueueRecord> = records
            .iter()
            .filter(|r| r.download_id == records[0].download_id)
            .collect();
        assert_eq!(pack.len(), 3);
        let episodes: Vec<Option<i64>> = pack.iter().map(|r| r.media_item()).collect();
        assert_eq!(episodes, [Some(101), Some(102), Some(103)]);
        let rejected = records
            .iter()
            .find(|r| r.tracked_download_state.as_deref() == Some("importPending"))
            .unwrap();
        assert!(rejected.messages().any(|m| m.starts_with("Not an upgrade")));
    }

    #[tokio::test]
    async fn follows_pages_until_every_record_is_read() {
        let server = MockServer::start().await;
        let mut first: serde_json::Value = serde_json::from_str(SONARR_QUEUE).unwrap();
        first["totalRecords"] = 6.into();
        let mut second: serde_json::Value = serde_json::from_str(RADARR_QUEUE).unwrap();
        second["page"] = 2.into();
        for (page, body) in [("1", first), ("2", second)] {
            Mock::given(method("GET"))
                .and(path("/sonarr/api/v3/queue"))
                .and(query_param("page", page))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
        }
        let records = client(Kind::Sonarr, &server).queue().await.unwrap();
        assert_eq!(records.len(), 6);
        assert_eq!(records[5].media_item(), Some(501));
    }

    #[tokio::test]
    async fn deletes_from_the_client_with_the_requested_blocklist_and_redownload() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/sonarr/api/v3/queue/1004"))
            .and(query_param("removeFromClient", "true"))
            .and(query_param("blocklist", "true"))
            .and(query_param("skipRedownload", "false"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        client(Kind::Sonarr, &server)
            .delete_queue_item(1004, true, false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn maps_each_series_to_its_quality_profile() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sonarr/api/v3/series"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"[{"id":1,"title":"Example Show","qualityProfileId":14},{"id":2,"title":"Other","qualityProfileId":7}]"#,
            ))
            .mount(&server)
            .await;
        let profiles = client(Kind::Sonarr, &server)
            .profiles_by_owner()
            .await
            .unwrap();
        assert_eq!(profiles, HashMap::from([(1, 14), (2, 7)]));
    }

    #[tokio::test]
    async fn surfaces_a_wrong_api_key_as_a_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sonarr/api/v3/qualityprofile"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let err = client(Kind::Sonarr, &server)
            .quality_profiles()
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::Status { status, .. } if status == reqwest::StatusCode::UNAUTHORIZED)
        );
    }
}
