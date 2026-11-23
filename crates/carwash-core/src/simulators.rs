//! Simulator devices, as `xcrun simctl list --json devices` reports them.
//!
//! carwash only ever asks `simctl` what exists and tells it what to erase or delete; the
//! device folders belong to the CoreSimulator service and are never removed directly.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

/// Lists the default device set.
pub const LIST: &[&str] = &["xcrun", "simctl", "list", "--json", "devices"];
/// Lists the clones Xcode keeps for parallel testing (`~/Library/Developer/XCTestDevices`).
pub const LIST_TESTING: &[&str] = &[
    "xcrun", "simctl", "--set", "testing", "list", "--json", "devices",
];

/// One simulator device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Simulator {
    pub udid: String,
    pub name: String,
    /// The runtime it runs, e.g. `iOS 27.0`.
    pub runtime: String,
    /// `simctl`'s word for what the device is doing: `Shutdown`, `Booted`, `Booting`...
    pub state: String,
    /// False when its runtime is no longer installed.
    pub available: bool,
    /// When it was last booted; `None` for a device that never was.
    pub last_used: Option<SystemTime>,
    pub data_path: PathBuf,
}

impl Simulator {
    /// Only a device that is shut down may be erased or deleted.
    pub fn is_shut_down(&self) -> bool {
        self.state == "Shutdown"
    }
}

#[derive(Deserialize)]
struct Listing {
    /// Devices by runtime identifier.
    devices: BTreeMap<String, Vec<Device>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Device {
    udid: String,
    name: String,
    state: String,
    #[serde(default)]
    is_available: bool,
    #[serde(default)]
    last_used_at: Option<String>,
    data_path: PathBuf,
}

/// Parses `simctl list --json devices`. Devices whose identifier is not a UUID are left
/// out: `simctl` reads words such as `all` and `booted` in its place.
pub fn parse(json: &[u8]) -> Result<Vec<Simulator>, serde_json::Error> {
    let listing: Listing = serde_json::from_slice(json)?;
    let mut simulators = Vec::new();
    for (runtime, devices) in listing.devices {
        let runtime = runtime_name(&runtime);
        for device in devices {
            if !is_udid(&device.udid) {
                continue;
            }
            simulators.push(Simulator {
                udid: device.udid,
                name: device.name,
                runtime: runtime.clone(),
                state: device.state,
                available: device.is_available,
                last_used: device
                    .last_used_at
                    .and_then(|at| at.parse::<jiff::Timestamp>().ok())
                    .and_then(|at| SystemTime::try_from(at).ok()),
                data_path: device.data_path,
            });
        }
    }
    Ok(simulators)
}

/// `8-4-4-4-12` hexadecimal digits.
pub fn is_udid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, len)| group.len() == len && group.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// `com.apple.CoreSimulator.SimRuntime.iOS-27-0` as `iOS 27.0`. Runtimes that are no longer
/// installed are known only by this identifier.
fn runtime_name(identifier: &str) -> String {
    let short = identifier.rsplit('.').next().unwrap_or(identifier);
    match short.split_once('-') {
        Some((platform, version)) => format!("{platform} {}", version.replace('-', ".")),
        None => short.to_owned(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    /// `xcrun simctl list --json devices` with names, identifiers and paths replaced. One
    /// device under a runtime that is not installed was added by hand.
    pub(crate) const LISTING: &str = include_str!("../tests/data/simctl-list-devices.json");
    /// `xcrun simctl --set testing list --json devices` on a machine with no test clones.
    pub(crate) const TESTING_LISTING: &str =
        include_str!("../tests/data/simctl-testing-list-devices.json");

    #[test]
    fn parses_the_device_list() {
        let simulators = parse(LISTING.as_bytes()).unwrap();
        assert_eq!(simulators.len(), 64);
        let booted: Vec<&Simulator> = simulators.iter().filter(|s| !s.is_shut_down()).collect();
        assert_eq!(booted.len(), 3);
        assert!(booted.iter().all(|s| s.state == "Booted"));

        let first = simulators
            .iter()
            .find(|s| s.udid == "00000001-0000-4000-8000-000000000001")
            .unwrap();
        assert_eq!(first.name, "iPhone 18 Pro");
        assert_eq!(first.runtime, "iOS 27.0");
        assert!(first.available);
        assert_eq!(
            first.last_used,
            Some(UNIX_EPOCH + Duration::from_secs(1_795_395_343)),
            "2026-11-23T00:55:43Z"
        );
        assert!(
            first
                .data_path
                .ends_with("00000001-0000-4000-8000-000000000001/data")
        );
        assert!(
            simulators.iter().any(|s| s.last_used.is_none()),
            "never booted"
        );
        assert!(simulators.iter().any(|s| s.runtime == "watchOS 26.5"));
    }

    #[test]
    fn devices_without_their_runtime_are_unavailable() {
        let simulators = parse(LISTING.as_bytes()).unwrap();
        let gone: Vec<&Simulator> = simulators.iter().filter(|s| !s.available).collect();
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].runtime, "iOS 17.5");
        assert_eq!(gone[0].name, "iPhone 15");
    }

    #[test]
    fn an_empty_device_set_parses() {
        assert!(parse(TESTING_LISTING.as_bytes()).unwrap().is_empty());
        assert!(parse(b"not json").is_err());
    }

    #[test]
    fn only_uuids_are_device_identifiers() {
        assert!(is_udid("5BB1785F-11BA-4A6A-9D2A-08C0255B8944"));
        for word in ["all", "booted", "unavailable", "", "../..", "5BB1785F"] {
            assert!(!is_udid(word), "{word}");
        }
        let listing = br#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-27-0":[
            {"udid":"all","name":"x","state":"Shutdown","isAvailable":true,"dataPath":"/x"}]}}"#;
        assert!(parse(listing).unwrap().is_empty());
    }

    #[test]
    fn names_runtimes_from_their_identifiers() {
        assert_eq!(
            runtime_name("com.apple.CoreSimulator.SimRuntime.iOS-27-0"),
            "iOS 27.0"
        );
        assert_eq!(
            runtime_name("com.apple.CoreSimulator.SimRuntime.watchOS-26-5"),
            "watchOS 26.5"
        );
        assert_eq!(runtime_name("custom"), "custom");
    }
}
