use std::net::SocketAddr;
use std::time::Duration;

use serde::{Deserialize, Deserializer};

use crate::arr::Kind;

/// The same patterns qBittorrent's `ExcludedFileNames` blocks in the
/// cluster: executables, scripts and installers for every desktop and mobile
/// platform.
pub const DEFAULT_EXECUTABLES: &[&str] = &[
    "*.exe",
    "*.com",
    "*.scr",
    "*.pif",
    "*.bat",
    "*.cmd",
    "*.msi",
    "*.msp",
    "*.msix",
    "*.msixbundle",
    "*.appx",
    "*.appxbundle",
    "*.appinstaller",
    "*.application",
    "*.appref-ms",
    "*.gadget",
    "*.dll",
    "*.sys",
    "*.ocx",
    "*.cpl",
    "*.xll",
    "*.lnk",
    "*.url",
    "*.scf",
    "*.inf",
    "*.reg",
    "*.chm",
    "*.hta",
    "*.msc",
    "*.sct",
    "*.vbs",
    "*.vbe",
    "*.js",
    "*.jse",
    "*.wsf",
    "*.wsh",
    "*.wsc",
    "*.ws",
    "*.ps1",
    "*.psm1",
    "*.jar",
    "*.jnlp",
    "*.py",
    "*.pyw",
    "*.pyz",
    "*.pyc",
    "*.sh",
    "*.run",
    "*.AppImage",
    "*.desktop",
    "*.deb",
    "*.rpm",
    "*.app",
    "*.dmg",
    "*.pkg",
    "*.mpkg",
    "*.command",
    "*.workflow",
    "*.scpt",
    "*.apk",
    "*.apks",
    "*.xapk",
    "*.apkm",
    "*.ipa",
];

pub const DEFAULT_MEDIA: &[&str] = &[
    "*.mkv", "*.mp4", "*.m4v", "*.avi", "*.mov", "*.wmv", "*.webm", "*.flv", "*.ts", "*.m2ts",
    "*.mpg", "*.mpeg", "*.vob", "*.iso",
];

pub const DEFAULT_FAILED_IMPORT: &[&str] = &[
    "^Not an upgrade for existing",
    "^Not a Custom Format upgrade for existing",
    "^No files found are eligible for import",
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read {path}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid config")]
    Parse(#[from] toml::de::Error),
    #[error("invalid config: {0}")]
    Invalid(String),
    #[error("environment variable {0} is not set")]
    MissingEnv(String),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "defaults::interval", with = "humantime_serde")]
    pub interval: Duration,
    #[serde(default = "defaults::yes")]
    pub dry_run: bool,
    #[serde(default = "defaults::max_removals")]
    pub max_removals_per_cycle: usize,
    #[serde(default = "defaults::listen")]
    pub listen: SocketAddr,
    pub qbit: QbitConfig,
    #[serde(default)]
    pub arr: Vec<ArrConfig>,
    #[serde(default)]
    pub rules: Rules,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QbitConfig {
    pub url: String,
    /// Categories managed without an arr. Removal there goes straight to
    /// qBittorrent, so nothing is blocklisted.
    #[serde(default)]
    pub categories: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArrConfig {
    pub kind: Kind,
    /// Includes the UrlBase; redirects are not followed.
    pub url: String,
    pub api_key_env: String,
    pub category: String,
}

impl ArrConfig {
    pub fn api_key(&self) -> Result<String, Error> {
        std::env::var(&self.api_key_env).map_err(|_| Error::MissingEnv(self.api_key_env.clone()))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Rules {
    pub fake: FakeRule,
    pub stalled: StalledRule,
    pub duplicate: DuplicateRule,
    pub failed_import: FailedImportRule,
    pub free_space: FreeSpaceRule,
    pub recover: RecoverRule,
    pub orphan: OrphanRule,
    pub budget: Budget,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FakeRule {
    pub enabled: bool,
    pub media: Vec<String>,
    pub executables: Vec<String>,
}

impl Default for FakeRule {
    fn default() -> Self {
        FakeRule {
            enabled: true,
            media: owned(DEFAULT_MEDIA),
            executables: owned(DEFAULT_EXECUTABLES),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StalledRule {
    pub enabled: bool,
    #[serde(with = "humantime_serde")]
    pub metadata_after: Duration,
    #[serde(with = "humantime_serde")]
    pub stalled_after: Duration,
}

impl Default for StalledRule {
    fn default() -> Self {
        StalledRule {
            enabled: true,
            metadata_after: Duration::from_secs(30 * 60),
            stalled_after: Duration::from_secs(3 * 60 * 60),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DuplicateRule {
    pub enabled: bool,
    /// Downloads allowed side by side for one episode or movie, so a slow
    /// first grab does not hold back a faster second one.
    pub keep: usize,
}

impl Default for DuplicateRule {
    fn default() -> Self {
        DuplicateRule {
            enabled: true,
            keep: 2,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FailedImportRule {
    pub enabled: bool,
    pub patterns: Vec<String>,
    pub research: bool,
}

impl Default for FailedImportRule {
    fn default() -> Self {
        FailedImportRule {
            enabled: true,
            patterns: owned(DEFAULT_FAILED_IMPORT),
            research: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FreeSpaceRule {
    pub enabled: bool,
    #[serde(deserialize_with = "bytes")]
    pub low: u64,
    #[serde(deserialize_with = "bytes")]
    pub high: u64,
}

impl Default for FreeSpaceRule {
    fn default() -> Self {
        FreeSpaceRule {
            enabled: true,
            low: 25 << 30,
            high: 50 << 30,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RecoverRule {
    pub enabled: bool,
    pub max_attempts: u32,
}

impl Default for RecoverRule {
    fn default() -> Self {
        RecoverRule {
            enabled: true,
            max_attempts: 3,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct OrphanRule {
    pub enabled: bool,
}

impl Default for OrphanRule {
    fn default() -> Self {
        OrphanRule { enabled: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Budget {
    pub max_researches_per_day: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            max_researches_per_day: 2,
        }
    }
}

impl Config {
    pub fn load(path: &str) -> Result<Config, Error> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })?;
        Config::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Config, Error> {
        let config: Config = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), Error> {
        let invalid = |msg: &str| Err(Error::Invalid(msg.to_owned()));
        if self.interval < Duration::from_secs(10) {
            return invalid("interval must be at least 10s");
        }
        if self.rules.duplicate.keep == 0 {
            return invalid("rules.duplicate.keep must be at least 1");
        }
        if self.rules.free_space.low > self.rules.free_space.high {
            return invalid("rules.free_space.low must not exceed rules.free_space.high");
        }
        let mut categories: Vec<&str> = self
            .arr
            .iter()
            .map(|a| a.category.as_str())
            .chain(self.qbit.categories.iter().map(String::as_str))
            .collect();
        if categories.is_empty() {
            return invalid("no category is managed: add an [[arr]] or qbit.categories");
        }
        categories.sort_unstable();
        if categories.windows(2).any(|w| w[0] == w[1]) {
            return invalid("each category may be managed only once");
        }
        Ok(())
    }

    /// Which arr owns a category; `None` for qBit-only categories and
    /// for categories the janitor does not manage at all.
    pub fn owner(&self, category: &str) -> Option<usize> {
        self.arr.iter().position(|a| a.category == category)
    }

    pub fn manages(&self, category: &str) -> bool {
        self.owner(category).is_some() || self.qbit.categories.iter().any(|c| c == category)
    }
}

/// `25G`-style sizes, binary multiples, or a plain byte count.
pub fn parse_bytes(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let (number, shift) = match text.char_indices().last() {
        Some((i, unit)) if unit.is_ascii_alphabetic() => {
            let shift = match unit.to_ascii_uppercase() {
                'K' => 10,
                'M' => 20,
                'G' => 30,
                'T' => 40,
                _ => return Err(format!("unknown size unit in {text:?}")),
            };
            (&text[..i], shift)
        }
        _ => (text, 0),
    };
    let number: u64 = number
        .trim()
        .parse()
        .map_err(|_| format!("invalid size {text:?}"))?;
    number
        .checked_mul(1 << shift)
        .ok_or_else(|| format!("size {text:?} is too large"))
}

fn bytes<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Size {
        Number(u64),
        Text(String),
    }
    match Size::deserialize(deserializer)? {
        Size::Number(n) => Ok(n),
        Size::Text(text) => parse_bytes(&text).map_err(serde::de::Error::custom),
    }
}

fn owned(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

mod defaults {
    use std::net::SocketAddr;
    use std::time::Duration;

    pub fn interval() -> Duration {
        Duration::from_secs(60)
    }

    pub fn yes() -> bool {
        true
    }

    pub fn max_removals() -> usize {
        10
    }

    pub fn listen() -> SocketAddr {
        SocketAddr::from(([0, 0, 0, 0], 9090))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
        [qbit]
        url = "http://qbittorrent:8080"

        [[arr]]
        kind = "sonarr"
        url = "http://sonarr:8989/sonarr"
        api_key_env = "SONARR_API_KEY"
        category = "tv-sonarr"
    "#;

    #[test]
    fn a_minimal_config_starts_safe() {
        let config = Config::parse(MINIMAL).unwrap();
        assert!(config.dry_run, "removal must be switched on explicitly");
        assert_eq!(config.interval, Duration::from_secs(60));
        assert_eq!(
            config.rules.stalled.metadata_after,
            Duration::from_secs(30 * 60)
        );
        assert_eq!(
            config.rules.stalled.stalled_after,
            Duration::from_secs(3 * 60 * 60)
        );
        assert_eq!(config.rules.duplicate.keep, 2);
        assert_eq!(config.rules.fake.executables.len(), 64);
        assert_eq!(config.owner("tv-sonarr"), Some(0));
        assert!(!config.manages("movies-radarr"));
    }

    #[test]
    fn the_example_config_is_valid() {
        let config = Config::parse(include_str!("../config.example.toml")).unwrap();
        assert_eq!(config.arr.len(), 2);
        assert_eq!(config.rules.free_space.low, 25 << 30);
    }

    #[test]
    fn typos_are_rejected_instead_of_ignored() {
        let text = format!("{MINIMAL}\n[rules.stalled]\nmetadata_aftr = \"1h\"\n");
        assert!(matches!(Config::parse(&text), Err(Error::Parse(_))));
    }

    #[test]
    fn free_space_marks_must_not_cross() {
        let text = format!("{MINIMAL}\n[rules.free_space]\nlow = \"50G\"\nhigh = \"25G\"\n");
        assert!(matches!(Config::parse(&text), Err(Error::Invalid(_))));
    }

    #[test]
    fn a_category_cannot_have_two_owners() {
        let text = format!(
            "{MINIMAL}\n[[arr]]\nkind = \"radarr\"\nurl = \"x\"\napi_key_env = \"K\"\ncategory = \"tv-sonarr\"\n"
        );
        assert!(matches!(Config::parse(&text), Err(Error::Invalid(_))));
    }

    #[test]
    fn sizes_use_binary_units() {
        assert_eq!(parse_bytes("25G"), Ok(25 << 30));
        assert_eq!(parse_bytes("512m"), Ok(512 << 20));
        assert_eq!(parse_bytes("1024"), Ok(1024));
        assert!(parse_bytes("5X").is_err());
        assert!(parse_bytes("G").is_err());
    }
}
