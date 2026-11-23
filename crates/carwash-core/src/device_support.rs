//! Device support folders (`~/Library/Developer/Xcode/iOS DeviceSupport`) and the devices
//! that still need them.
//!
//! Xcode copies symbols from a device the first time it connects on a given OS build, into
//! a folder named for the device model, OS version and build. Nothing removes the folders
//! of builds a device has since moved past. `xcrun devicectl list devices` says which
//! devices this Mac knows, what they run and when they last connected, which is enough to
//! tell the folders still in use from the ones left behind.

use crate::caches::Advice;
use serde::Deserialize;
use std::cmp::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Lists the devices CoreDevice knows as JSON on standard output.
pub const LIST: &[&str] = &[
    "xcrun",
    "devicectl",
    "list",
    "devices",
    "--quiet",
    "--timeout",
    "10",
    "--json-output",
    "/dev/stdout",
];

/// A device counts as still connected to this Mac for this long after it last was.
pub const RECENT: Duration = Duration::from_secs(90 * 86_400);

/// Seconds from the Unix epoch to 2001-01-01, which Apple's timestamps count from.
const APPLE_EPOCH: u64 = 978_307_200;

/// What a device support folder's name says: `iPad16,3 27.3 (24C5037i)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Support {
    /// The device model, e.g. `iPad16,3`. Folders written by old versions of Xcode have
    /// none: they were shared by every model.
    pub model: Option<String>,
    /// The OS version's numbers, e.g. `[27, 3]`.
    pub version: Vec<u32>,
    pub build: String,
}

impl Support {
    /// Parses a folder name: an optional model, a version, a build in parentheses, and
    /// (from old versions of Xcode) an architecture after it.
    pub fn parse(name: &str) -> Option<Self> {
        let (before, rest) = name.split_once(" (")?;
        let (build, _arch) = rest.split_once(')')?;
        let (model, version) = match before.rsplit_once(' ') {
            Some((model, version)) => (Some(model.trim()), version),
            None => (None, before),
        };
        let version: Vec<u32> = version
            .split('.')
            .map(|part| part.parse().ok())
            .collect::<Option<_>>()?;
        if build.is_empty() || model.is_some_and(str::is_empty) {
            return None;
        }
        Some(Self {
            model: model.map(str::to_owned),
            version,
            build: build.to_owned(),
        })
    }

    /// The OS version as written: `27.3`.
    pub fn version_text(&self) -> String {
        let version: Vec<String> = self.version.iter().map(u32::to_string).collect();
        version.join(".")
    }

    /// `27.3 (24C5037i)`.
    fn label(&self) -> String {
        format!("{} ({})", self.version_text(), self.build)
    }

    fn newer_than(&self, other: &Self) -> bool {
        match self.version.cmp(&other.version) {
            Ordering::Equal => self.build > other.build,
            order => order == Ordering::Greater,
        }
    }
}

/// A physical device this Mac has paired with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    /// The model, e.g. `iPad16,3`.
    pub model: String,
    /// The OS version it runs, e.g. `27.3`.
    pub os_version: String,
    pub build: Option<String>,
    pub last_connected: Option<SystemTime>,
}

impl Device {
    fn connected_recently(&self, now: SystemTime) -> bool {
        self.last_connected
            .is_some_and(|at| now.duration_since(at).unwrap_or_default() <= RECENT)
    }

    fn runs(&self, support: &Support) -> bool {
        if support.model.as_deref() != Some(self.model.as_str()) {
            return false;
        }
        match &self.build {
            Some(build) => *build == support.build,
            None => {
                let version: Option<Vec<u32>> = self
                    .os_version
                    .split('.')
                    .map(|part| part.parse().ok())
                    .collect();
                version.as_ref() == Some(&support.version)
            }
        }
    }
}

#[derive(Deserialize)]
struct Output {
    result: Listing,
}

#[derive(Deserialize)]
struct Listing {
    #[serde(default)]
    devices: Vec<Entry>,
}

/// One device. `properties` holds everything in current versions of devicectl; the three
/// older dictionaries are deprecated but still what earlier versions print.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    #[serde(default)]
    properties: Option<Properties>,
    #[serde(default)]
    hardware_properties: Option<Hardware>,
    #[serde(default)]
    device_properties: Option<OldDevice>,
    #[serde(default)]
    connection_properties: Option<OldConnection>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Properties {
    hardware: Hardware,
    software: Software,
    connection: Connection,
    state: State,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Hardware {
    product_type: Option<String>,
    /// `physical` or `simulated`.
    reality: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Software {
    os_version_number: Option<Named>,
    os_build_versions: Option<Builds>,
}

/// A version: `stringValue` for an OS version, `name` for a build.
#[derive(Deserialize)]
struct Named {
    #[serde(default, rename = "stringValue", alias = "name")]
    text: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Builds {
    build_version: Option<Named>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Connection {
    /// Seconds since 2001-01-01.
    last_connection_date: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct State {
    name: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct OldDevice {
    name: Option<String>,
    os_version_number: Option<String>,
    os_build_update: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct OldConnection {
    /// An ISO 8601 timestamp.
    last_connection_date: Option<String>,
}

/// Parses `devicectl list devices` JSON, keeping the physical devices. Simulators are in
/// the same list and have no device support.
pub fn parse(json: &[u8]) -> Result<Vec<Device>, serde_json::Error> {
    let output: Output = serde_json::from_slice(json)?;
    let mut devices = Vec::new();
    for entry in output.result.devices {
        let new = entry.properties.unwrap_or_default();
        let old_hardware = entry.hardware_properties.unwrap_or_default();
        let old_device = entry.device_properties.unwrap_or_default();
        let old_connection = entry.connection_properties.unwrap_or_default();
        let reality = new.hardware.reality.or(old_hardware.reality);
        if reality.as_deref() != Some("physical") {
            continue;
        }
        let model = new.hardware.product_type.or(old_hardware.product_type);
        let os_version = new
            .software
            .os_version_number
            .and_then(|v| v.text)
            .or(old_device.os_version_number);
        let (Some(model), Some(os_version)) = (model, os_version) else {
            continue;
        };
        let last_connected = new
            .connection
            .last_connection_date
            .filter(|secs| secs.is_finite() && *secs >= 0.0)
            .map(|secs| UNIX_EPOCH + Duration::from_secs(APPLE_EPOCH + secs as u64))
            .or_else(|| {
                old_connection
                    .last_connection_date
                    .and_then(|at| at.parse::<jiff::Timestamp>().ok())
                    .and_then(|at| SystemTime::try_from(at).ok())
            });
        devices.push(Device {
            name: new
                .state
                .name
                .or(old_device.name)
                .unwrap_or_else(|| model.clone()),
            build: new
                .software
                .os_build_versions
                .and_then(|b| b.build_version)
                .and_then(|b| b.text)
                .or(old_device.os_build_update),
            model,
            os_version,
            last_connected,
        });
    }
    Ok(devices)
}

/// What to make of each folder in `names`, in the same order.
///
/// For every device model, the newest folder is kept, and so is any folder a device that
/// connected in the last [`RECENT`] days runs right now. Older folders of the same model
/// are recommended for removal. The only folder of a model is its newest, so it is never
/// recommended away. Folders whose name cannot be read, or that name no model, get no advice.
pub fn advise(names: &[&str], devices: &[Device], now: SystemTime) -> Vec<Option<Advice>> {
    let folders: Vec<Option<Support>> = names.iter().map(|name| Support::parse(name)).collect();
    folders
        .iter()
        .map(|folder| {
            let folder = folder.as_ref()?;
            let model = folder.model.as_deref()?;
            let in_use = devices
                .iter()
                .find(|d| d.connected_recently(now) && d.runs(folder));
            if let Some(device) = in_use {
                return Some(Advice::keep(format!("{} runs it", device.name)));
            }
            let newest = folders
                .iter()
                .flatten()
                .filter(|other| other.model.as_deref() == Some(model))
                .fold(folder, |newest, other| {
                    if other.newer_than(newest) {
                        other
                    } else {
                        newest
                    }
                });
            if newest == folder {
                let connects = devices
                    .iter()
                    .any(|d| d.model == model && d.connected_recently(now));
                Some(Advice::keep(if connects {
                    format!("newest for {model}, which still connects")
                } else {
                    format!("newest for {model}")
                }))
            } else {
                Some(Advice::remove(format!("superseded by {}", newest.label())))
            }
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::caches::Verdict;

    /// `xcrun devicectl list devices --json-output` with two physical devices and two
    /// simulators; names, identifiers, serial numbers and addresses replaced.
    pub(crate) const LISTING: &str = include_str!("../tests/data/devicectl-list-devices.json");

    /// The folders on the machine the listing was captured on.
    pub(crate) const FOLDERS: [&str; 5] = [
        "iPad16,3 26.7 (23H24)",
        "iPad16,3 27.2 (24B119)",
        "iPad16,3 27.3 (24C5037i)",
        "iPhone14,3 27.0.1 (24A446)",
        "iPhone14,3 27.2.1 (24B156)",
    ];

    /// A day after the capture.
    pub(crate) fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_795_394_400 + 86_400)
    }

    #[test]
    fn parses_folder_names() {
        assert_eq!(
            Support::parse("iPad16,3 27.3 (24C5037i)"),
            Some(Support {
                model: Some("iPad16,3".into()),
                version: vec![27, 3],
                build: "24C5037i".into(),
            })
        );
        let old = Support::parse("16.4 (20E247) arm64e").unwrap();
        assert_eq!(old.model, None);
        assert_eq!(old.version, [16, 4]);
        assert_eq!(old.build, "20E247");
        let watch = Support::parse("Watch6,2 10.1.1 (21S71)").unwrap();
        assert_eq!(watch.model.as_deref(), Some("Watch6,2"));
        assert_eq!(watch.version, [10, 1, 1]);
        for name in [
            "",
            "DerivedData",
            "iPad16,3 27.3",
            "iPad16,3 beta (24C)",
            "27.3 ()",
        ] {
            assert_eq!(Support::parse(name), None, "{name}");
        }
    }

    #[test]
    fn parses_physical_devices_from_devicectl() {
        let devices = parse(LISTING.as_bytes()).unwrap();
        assert_eq!(devices.len(), 2, "simulators are left out");
        let ipad = &devices[0];
        assert_eq!(ipad.name, "Test iPad");
        assert_eq!(ipad.model, "iPad16,3");
        assert_eq!(ipad.os_version, "27.3");
        assert_eq!(ipad.build.as_deref(), Some("24C5037i"));
        assert_eq!(
            ipad.last_connected,
            Some(UNIX_EPOCH + Duration::from_secs(1_795_394_400)),
            "2026-11-23T00:40:00Z"
        );
        let iphone = &devices[1];
        assert_eq!(iphone.model, "iPhone14,3");
        assert_eq!(iphone.os_version, "27.2.1");
        assert_eq!(iphone.build.as_deref(), Some("24B156"));
    }

    #[test]
    fn reads_the_deprecated_dictionaries_when_properties_is_missing() {
        let mut doc: serde_json::Value = serde_json::from_str(LISTING).unwrap();
        for device in doc["result"]["devices"].as_array_mut().unwrap() {
            device.as_object_mut().unwrap().remove("properties");
        }
        let devices = parse(doc.to_string().as_bytes()).unwrap();
        assert_eq!(devices, parse(LISTING.as_bytes()).unwrap());
        assert!(parse(b"{}").is_err());
        assert!(parse(br#"{"result":{}}"#).unwrap().is_empty());
    }

    fn verdicts(names: &[&str], devices: &[Device], now: SystemTime) -> Vec<(Verdict, String)> {
        advise(names, devices, now)
            .into_iter()
            .map(|advice| {
                let advice = advice.expect("advice");
                (advice.verdict, advice.reason)
            })
            .collect()
    }

    #[test]
    fn keeps_what_connected_devices_run_and_recommends_older_folders() {
        let devices = parse(LISTING.as_bytes()).unwrap();
        assert_eq!(
            verdicts(&FOLDERS, &devices, now()),
            [
                (Verdict::Remove, "superseded by 27.3 (24C5037i)".into()),
                (Verdict::Remove, "superseded by 27.3 (24C5037i)".into()),
                (Verdict::Keep, "Test iPad runs it".into()),
                (Verdict::Remove, "superseded by 27.2.1 (24B156)".into()),
                (Verdict::Keep, "Test iPhone runs it".into()),
            ]
        );
    }

    #[test]
    fn the_newest_folder_of_every_model_is_kept_whoever_connects() {
        // No devicectl, or no devices: only the newest per model is known to matter.
        assert_eq!(
            verdicts(&FOLDERS, &[], now()),
            [
                (Verdict::Remove, "superseded by 27.3 (24C5037i)".into()),
                (Verdict::Remove, "superseded by 27.3 (24C5037i)".into()),
                (Verdict::Keep, "newest for iPad16,3".into()),
                (Verdict::Remove, "superseded by 27.2.1 (24B156)".into()),
                (Verdict::Keep, "newest for iPhone14,3".into()),
            ]
        );
        // The only folder of a model is never recommended away, connected or not.
        let devices = parse(LISTING.as_bytes()).unwrap();
        for known in [&devices[..], &[]] {
            let only = verdicts(&["iPhone14,3 27.0.1 (24A446)"], known, now());
            assert_eq!(only[0].0, Verdict::Keep, "{only:?}");
        }
        assert_eq!(
            verdicts(&["iPhone14,3 27.0.1 (24A446)"], &devices, now())[0].1,
            "newest for iPhone14,3, which still connects"
        );
    }

    #[test]
    fn a_device_still_on_an_older_build_keeps_that_build() {
        let mut devices = parse(LISTING.as_bytes()).unwrap();
        devices[0].os_version = "27.2".into();
        devices[0].build = Some("24B119".into());
        let ipad = verdicts(&FOLDERS[..3], &devices, now());
        assert_eq!(ipad[0].0, Verdict::Remove);
        assert_eq!(ipad[1], (Verdict::Keep, "Test iPad runs it".into()));
        assert_eq!(ipad[2].0, Verdict::Keep, "and the newest stays too");
        // Without a build, the version decides.
        devices[0].build = None;
        assert_eq!(verdicts(&FOLDERS[..3], &devices, now())[1].0, Verdict::Keep);
    }

    #[test]
    fn a_device_not_seen_for_months_no_longer_holds_its_folder() {
        let devices = parse(LISTING.as_bytes()).unwrap();
        let later = now() + RECENT + Duration::from_secs(86_400);
        let mut folders = FOLDERS.to_vec();
        folders.push("iPad16,3 27.4 (24D60)");
        let ipad = verdicts(&folders, &devices, later);
        assert_eq!(
            ipad[2].0,
            Verdict::Remove,
            "27.3, which the iPad ran long ago"
        );
        assert_eq!(ipad[5], (Verdict::Keep, "newest for iPad16,3".into()));
        // While it still connects, both stay.
        let ipad = verdicts(&folders, &devices, now());
        assert_eq!(ipad[2].0, Verdict::Keep);
        assert_eq!(
            ipad[5],
            (
                Verdict::Keep,
                "newest for iPad16,3, which still connects".into()
            )
        );
    }

    #[test]
    fn folders_without_a_model_get_no_advice() {
        let advice = advise(&["16.4 (20E247)", "15.0 (19A346)", "notes"], &[], now());
        assert_eq!(advice, [None, None, None]);
    }

    #[test]
    fn builds_break_ties_between_equal_versions() {
        let betas = ["iPad16,3 27.3 (24C5037i)", "iPad16,3 27.3 (24C5040a)"];
        let kept = verdicts(&betas, &[], now());
        assert_eq!(kept[0].0, Verdict::Remove);
        assert_eq!(kept[1].0, Verdict::Keep);
    }
}
