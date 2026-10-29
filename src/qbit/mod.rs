//! The slice of the qBittorrent WebAPI the janitor uses. In the cluster the
//! WebUI skips authentication for in-cluster clients, so there is no login.

mod model;

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use serde::de::DeserializeOwned;

pub use model::{File, MainData, State, Torrent};

/// `torrents/stop` and `torrents/start` replaced `pause`/`resume` in 2.11.
/// Older servers would 404 on every free-space action, so refuse them.
pub const MIN_API: Version = Version(2, 11, 0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl FromStr for Version {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || Error::BadVersion(s.to_owned());
        let mut parts = s
            .trim()
            .split('.')
            .map(|p| p.parse::<u32>().map_err(|_| bad()));
        let major = parts.next().ok_or_else(bad)??;
        let minor = parts.next().unwrap_or(Ok(0))?;
        let patch = parts.next().unwrap_or(Ok(0))?;
        Ok(Version(major, minor, patch))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("qBittorrent request to {path} failed")]
    Request {
        path: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("qBittorrent answered {status} to {path}")]
    Status {
        path: &'static str,
        status: reqwest::StatusCode,
    },
    #[error(
        "qBittorrent WebAPI {0} is older than {MIN_API}, which added torrents/stop and torrents/start"
    )]
    Unsupported(Version),
    #[error("qBittorrent reported an unreadable WebAPI version {0:?}")]
    BadVersion(String),
}

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
}

#[derive(Deserialize)]
struct RawMainData {
    #[serde(default)]
    torrents: HashMap<String, Torrent>,
    #[serde(default)]
    server_state: Option<RawServerState>,
}

#[derive(Deserialize)]
struct RawServerState {
    free_space_on_disk: Option<u64>,
}

impl Client {
    pub fn new(base: &str, http: reqwest::Client) -> Self {
        Client {
            http,
            base: base.trim_end_matches('/').to_owned(),
        }
    }

    pub async fn check_version(&self) -> Result<Version, Error> {
        let path = "/api/v2/app/webapiVersion";
        let version: Version = self
            .send(self.http.get(self.url(path)), path)
            .await?
            .text()
            .await
            .map_err(|source| Error::Request { path, source })?
            .parse()?;
        if version < MIN_API {
            return Err(Error::Unsupported(version));
        }
        Ok(version)
    }

    /// Always a full update (rid=0), so every call is one consistent snapshot
    /// of all torrents plus the server state.
    pub async fn main_data(&self) -> Result<MainData, Error> {
        let raw: RawMainData = self
            .get_json("/api/v2/sync/maindata", &[("rid", "0")])
            .await?;
        let mut torrents: Vec<Torrent> = raw
            .torrents
            .into_iter()
            .map(|(hash, mut torrent)| {
                torrent.hash = hash;
                torrent
            })
            .collect();
        torrents.sort_by(|a, b| a.hash.cmp(&b.hash));
        Ok(MainData {
            torrents,
            free_space: raw.server_state.and_then(|s| s.free_space_on_disk),
        })
    }

    pub async fn files(&self, hash: &str) -> Result<Vec<File>, Error> {
        self.get_json("/api/v2/torrents/files", &[("hash", hash)])
            .await
    }

    pub async fn delete(&self, hashes: &[String], delete_files: bool) -> Result<(), Error> {
        let files = if delete_files { "true" } else { "false" };
        self.post(
            "/api/v2/torrents/delete",
            &[("hashes", &hashes.join("|")), ("deleteFiles", files)],
        )
        .await
    }

    pub async fn stop(&self, hashes: &[String]) -> Result<(), Error> {
        self.post("/api/v2/torrents/stop", &[("hashes", &hashes.join("|"))])
            .await
    }

    pub async fn start(&self, hashes: &[String]) -> Result<(), Error> {
        self.post("/api/v2/torrents/start", &[("hashes", &hashes.join("|"))])
            .await
    }

    pub async fn add_tag(&self, hashes: &[String], tag: &str) -> Result<(), Error> {
        self.post(
            "/api/v2/torrents/addTags",
            &[("hashes", &hashes.join("|")), ("tags", tag)],
        )
        .await
    }

    pub async fn remove_tag(&self, hashes: &[String], tag: &str) -> Result<(), Error> {
        self.post(
            "/api/v2/torrents/removeTags",
            &[("hashes", &hashes.join("|")), ("tags", tag)],
        )
        .await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &'static str,
        query: &[(&str, &str)],
    ) -> Result<T, Error> {
        let request = self.http.get(self.url(path)).query(query);
        self.send(request, path)
            .await?
            .json()
            .await
            .map_err(|source| Error::Request { path, source })
    }

    async fn post(&self, path: &'static str, form: &[(&str, &str)]) -> Result<(), Error> {
        self.send(self.http.post(self.url(path)).form(form), path)
            .await
            .map(drop)
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        path: &'static str,
    ) -> Result<reqwest::Response, Error> {
        let response = request
            .send()
            .await
            .map_err(|source| Error::Request { path, source })?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Status { path, status });
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const MAINDATA: &str = include_str!("../../tests/fixtures/qbit/maindata.json");
    const FILES: &str = include_str!("../../tests/fixtures/qbit/files.json");

    async fn server_with(verb: &str, at: &str, response: ResponseTemplate) -> (MockServer, Client) {
        let server = MockServer::start().await;
        Mock::given(method(verb))
            .and(path(at))
            .respond_with(response)
            .mount(&server)
            .await;
        let client = Client::new(&server.uri(), reqwest::Client::new());
        (server, client)
    }

    #[tokio::test]
    async fn accepts_the_webapi_that_has_stop_and_start() {
        let (_s, client) = server_with(
            "GET",
            "/api/v2/app/webapiVersion",
            ResponseTemplate::new(200).set_body_string("2.16.2"),
        )
        .await;
        assert_eq!(client.check_version().await.unwrap(), Version(2, 16, 2));
    }

    #[tokio::test]
    async fn refuses_a_webapi_that_only_has_pause_and_resume() {
        let (_s, client) = server_with(
            "GET",
            "/api/v2/app/webapiVersion",
            ResponseTemplate::new(200).set_body_string("2.9.3"),
        )
        .await;
        assert!(matches!(
            client.check_version().await,
            Err(Error::Unsupported(Version(2, 9, 3)))
        ));
    }

    #[tokio::test]
    async fn reads_every_torrent_with_its_hash_and_the_free_space() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/sync/maindata"))
            .and(query_param("rid", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_string(MAINDATA))
            .mount(&server)
            .await;
        let data = Client::new(&server.uri(), reqwest::Client::new())
            .main_data()
            .await
            .unwrap();

        assert_eq!(data.free_space, Some(26_843_545_600));
        assert_eq!(data.torrents.len(), 4);
        assert!(data.torrents.iter().all(|t| t.hash.len() == 40));
        let metadata = data
            .torrents
            .iter()
            .find(|t| t.state == State::MetaDL)
            .unwrap();
        assert_eq!(
            metadata.name, metadata.hash,
            "a torrent without metadata is named by its hash"
        );
        let states: Vec<State> = data.torrents.iter().map(|t| t.state).collect();
        for expected in [
            State::MetaDL,
            State::StalledDL,
            State::Error,
            State::StalledUP,
        ] {
            assert!(states.contains(&expected), "{expected:?} missing");
        }
    }

    #[tokio::test]
    async fn reads_file_names_and_priorities() {
        let (_s, client) = server_with(
            "GET",
            "/api/v2/torrents/files",
            ResponseTemplate::new(200).set_body_string(FILES),
        )
        .await;
        let files = client.files("abc").await.unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[2].priority, 0);
        assert!(files[2].name.ends_with(".exe"));
    }

    #[tokio::test]
    async fn stops_with_hashes_joined_by_pipes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/torrents/stop"))
            .and(body_string("hashes=aa%7Cbb"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let client = Client::new(&server.uri(), reqwest::Client::new());
        client.stop(&["aa".into(), "bb".into()]).await.unwrap();
    }

    #[tokio::test]
    async fn deletes_with_files_when_asked() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/torrents/delete"))
            .and(body_string("hashes=aa&deleteFiles=true"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let client = Client::new(&server.uri(), reqwest::Client::new());
        client.delete(&["aa".into()], true).await.unwrap();
    }

    #[tokio::test]
    async fn reports_a_rejected_request_with_its_status() {
        let (_s, client) =
            server_with("POST", "/api/v2/torrents/start", ResponseTemplate::new(404)).await;
        let err = client.start(&["aa".into()]).await.unwrap_err();
        assert!(
            matches!(err, Error::Status { status, .. } if status == reqwest::StatusCode::NOT_FOUND)
        );
    }

    #[test]
    fn versions_compare_numerically() {
        assert!("2.9.0".parse::<Version>().unwrap() < "2.11.0".parse::<Version>().unwrap());
        assert!("2.11".parse::<Version>().unwrap() >= MIN_API);
        assert!("two".parse::<Version>().is_err());
    }
}
