//! The calls the janitor makes, as traits: the cycle and the executor run
//! against the real clients in production and small fakes in tests.

use std::collections::HashMap;
use std::future::Future;

use crate::arr::{self, Kind, QualityProfile, QueueRecord};
use crate::qbit::{self, File, MainData};

pub trait Qbit {
    fn main_data(&self) -> impl Future<Output = Result<MainData, qbit::Error>>;
    fn files(&self, hash: &str) -> impl Future<Output = Result<Vec<File>, qbit::Error>>;
    /// Removes the torrents and their data.
    fn delete(&self, hashes: &[String]) -> impl Future<Output = Result<(), qbit::Error>>;
    fn stop(&self, hashes: &[String]) -> impl Future<Output = Result<(), qbit::Error>>;
    fn start(&self, hashes: &[String]) -> impl Future<Output = Result<(), qbit::Error>>;
    fn add_tag(
        &self,
        hashes: &[String],
        tag: &str,
    ) -> impl Future<Output = Result<(), qbit::Error>>;
    fn remove_tag(
        &self,
        hashes: &[String],
        tag: &str,
    ) -> impl Future<Output = Result<(), qbit::Error>>;
}

pub trait Arr {
    fn kind(&self) -> Kind;
    fn queue(&self) -> impl Future<Output = Result<Vec<QueueRecord>, arr::Error>>;
    fn quality_profiles(&self) -> impl Future<Output = Result<Vec<QualityProfile>, arr::Error>>;
    fn profiles_by_owner(&self) -> impl Future<Output = Result<HashMap<i64, i64>, arr::Error>>;
    fn delete_queue_item(
        &self,
        id: i64,
        blocklist: bool,
        skip_redownload: bool,
    ) -> impl Future<Output = Result<(), arr::Error>>;
}

impl Qbit for qbit::Client {
    fn main_data(&self) -> impl Future<Output = Result<MainData, qbit::Error>> {
        qbit::Client::main_data(self)
    }

    fn files(&self, hash: &str) -> impl Future<Output = Result<Vec<File>, qbit::Error>> {
        qbit::Client::files(self, hash)
    }

    fn delete(&self, hashes: &[String]) -> impl Future<Output = Result<(), qbit::Error>> {
        qbit::Client::delete(self, hashes, true)
    }

    fn stop(&self, hashes: &[String]) -> impl Future<Output = Result<(), qbit::Error>> {
        qbit::Client::stop(self, hashes)
    }

    fn start(&self, hashes: &[String]) -> impl Future<Output = Result<(), qbit::Error>> {
        qbit::Client::start(self, hashes)
    }

    fn add_tag(
        &self,
        hashes: &[String],
        tag: &str,
    ) -> impl Future<Output = Result<(), qbit::Error>> {
        qbit::Client::add_tag(self, hashes, tag)
    }

    fn remove_tag(
        &self,
        hashes: &[String],
        tag: &str,
    ) -> impl Future<Output = Result<(), qbit::Error>> {
        qbit::Client::remove_tag(self, hashes, tag)
    }
}

impl Arr for arr::Client {
    fn kind(&self) -> Kind {
        arr::Client::kind(self)
    }

    fn queue(&self) -> impl Future<Output = Result<Vec<QueueRecord>, arr::Error>> {
        arr::Client::queue(self)
    }

    fn quality_profiles(&self) -> impl Future<Output = Result<Vec<QualityProfile>, arr::Error>> {
        arr::Client::quality_profiles(self)
    }

    fn profiles_by_owner(&self) -> impl Future<Output = Result<HashMap<i64, i64>, arr::Error>> {
        arr::Client::profiles_by_owner(self)
    }

    fn delete_queue_item(
        &self,
        id: i64,
        blocklist: bool,
        skip_redownload: bool,
    ) -> impl Future<Output = Result<(), arr::Error>> {
        arr::Client::delete_queue_item(self, id, blocklist, skip_redownload)
    }
}
