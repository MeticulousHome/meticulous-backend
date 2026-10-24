//! Firmware versions: `Machine._parseVersionString` with the PEP 440
//! `packaging.version.Version` it relies on, and the version stamp
//! `ESPToolWrapper.get_version_from_firmware` reads from a firmware image.

use std::cmp::Ordering;
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;

use crate::pynum::py_strip;

/// A non-negative decimal integer of any size, as Python's `int` holds it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Dec(String);

impl Dec {
    fn parse(digits: &str) -> Dec {
        let trimmed = digits.trim_start_matches('0');
        Dec(if trimmed.is_empty() { "0" } else { trimmed }.to_string())
    }

    fn zero() -> Dec {
        Dec("0".to_string())
    }

    fn is_zero(&self) -> bool {
        self.0 == "0"
    }
}

impl Ord for Dec {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.cmp(&other.0))
    }
}

impl PartialOrd for Dec {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Dec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `packaging`'s `NegativeInfinity` / value / `Infinity` in sort keys.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Bound<T> {
    NegInf,
    Val(T),
    Inf,
}

/// A local version part: alphanumeric parts sort before numeric ones.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum LocalPart {
    Str(String),
    Int(Dec),
}

type CmpKey = (
    Dec,
    Vec<Dec>,
    Bound<(String, Dec)>,
    Bound<(String, Dec)>,
    Bound<(String, Dec)>,
    Bound<Vec<LocalPart>>,
);

/// The `InvalidVersion` that `packaging.version.Version` raises.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Invalid version: '{0}'")]
pub struct InvalidVersion(pub String);

/// `packaging.version.Version` (packaging 24): a PEP 440 version, compared
/// and hashed by its normalized key, so `1.0 == 1.0.0` and `1.0a == 1.0a0`.
#[derive(Debug, Clone)]
pub struct Pep440Version {
    epoch: Dec,
    release: Vec<Dec>,
    pre: Option<(String, Dec)>,
    post: Option<Dec>,
    dev: Option<Dec>,
    local: Option<Vec<LocalPart>>,
}

// packaging's VERSION_PATTERN, compiled with re.VERBOSE | re.IGNORECASE and
// anchored by `^\s*...\s*$`.
static VERSION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        ^[\s\x1C-\x1F]*
        v?
        (?:
            (?:(?P<epoch>[0-9]+)!)?
            (?P<release>[0-9]+(?:\.[0-9]+)*)
            (?P<pre>
                [-_\.]?
                (?P<pre_l>alpha|a|beta|b|preview|pre|c|rc)
                [-_\.]?
                (?P<pre_n>[0-9]+)?
            )?
            (?P<post>
                (?:-(?P<post_n1>[0-9]+))
                |
                (?:
                    [-_\.]?
                    (?P<post_l>post|rev|r)
                    [-_\.]?
                    (?P<post_n2>[0-9]+)?
                )
            )?
            (?P<dev>
                [-_\.]?
                (?P<dev_l>dev)
                [-_\.]?
                (?P<dev_n>[0-9]+)?
            )?
        )
        (?:\+(?P<local>[a-z0-9]+(?:[-_\.][a-z0-9]+)*))?
        [\s\x1C-\x1F]*$",
    )
    .expect("the PEP 440 pattern is a valid regex")
});

impl Pep440Version {
    /// `Version(text)`.
    pub fn parse(text: &str) -> Result<Pep440Version, InvalidVersion> {
        let caps = VERSION_REGEX
            .captures(text)
            .ok_or_else(|| InvalidVersion(text.to_string()))?;
        let group = |name: &str| caps.name(name).map(|m| m.as_str());
        let letter = |letter: &str| -> String {
            match letter.to_lowercase().as_str() {
                "alpha" => "a".to_string(),
                "beta" => "b".to_string(),
                "c" | "pre" | "preview" => "rc".to_string(),
                other => other.to_string(),
            }
        };
        let release_text = group("release").unwrap_or("0");
        Ok(Pep440Version {
            epoch: group("epoch").map_or_else(Dec::zero, Dec::parse),
            release: release_text.split('.').map(Dec::parse).collect(),
            pre: group("pre_l")
                .map(|l| (letter(l), group("pre_n").map_or_else(Dec::zero, Dec::parse))),
            post: if group("post_l").is_some() || group("post_n1").is_some() {
                Some(
                    group("post_n1")
                        .or(group("post_n2"))
                        .map_or_else(Dec::zero, Dec::parse),
                )
            } else {
                None
            },
            dev: group("dev_l").map(|_| group("dev_n").map_or_else(Dec::zero, Dec::parse)),
            local: group("local").map(|local| {
                local
                    .split(['.', '_', '-'])
                    .map(|part| {
                        if !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) {
                            LocalPart::Int(Dec::parse(part))
                        } else {
                            LocalPart::Str(part.to_lowercase())
                        }
                    })
                    .collect()
            }),
        })
    }

    fn key(&self) -> CmpKey {
        let mut release = self.release.clone();
        while release.last().is_some_and(Dec::is_zero) {
            release.pop();
        }
        let pre = match (&self.pre, &self.post, &self.dev) {
            (None, None, Some(_)) => Bound::NegInf,
            (None, _, _) => Bound::Inf,
            (Some(pre), _, _) => Bound::Val(pre.clone()),
        };
        let post = self
            .post
            .clone()
            .map_or(Bound::NegInf, |n| Bound::Val(("post".to_string(), n)));
        let dev = self
            .dev
            .clone()
            .map_or(Bound::Inf, |n| Bound::Val(("dev".to_string(), n)));
        let local = self.local.clone().map_or(Bound::NegInf, Bound::Val);
        (self.epoch.clone(), release, pre, post, dev, local)
    }
}

impl PartialEq for Pep440Version {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for Pep440Version {}

impl Ord for Pep440Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

impl PartialOrd for Pep440Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Pep440Version {
    /// `str(Version)`: the normalized form.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.epoch.is_zero() {
            write!(f, "{}!", self.epoch)?;
        }
        let release: Vec<String> = self.release.iter().map(ToString::to_string).collect();
        f.write_str(&release.join("."))?;
        if let Some((letter, number)) = &self.pre {
            write!(f, "{letter}{number}")?;
        }
        if let Some(post) = &self.post {
            write!(f, ".post{post}")?;
        }
        if let Some(dev) = &self.dev {
            write!(f, ".dev{dev}")?;
        }
        if let Some(local) = &self.local {
            let parts: Vec<String> = local
                .iter()
                .map(|part| match part {
                    LocalPart::Str(s) => s.clone(),
                    LocalPart::Int(n) => n.to_string(),
                })
                .collect();
            write!(f, "+{}", parts.join("."))?;
        }
        Ok(())
    }
}

/// The `ExtraCommits` entry of a parsed firmware version: the int `0` when
/// the version has no `-` part, else the text of that part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtraCommits {
    /// The default `0` (an int).
    Default,
    /// The second `-` component, as a string.
    Text(String),
}

impl fmt::Display for ExtraCommits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtraCommits::Default => f.write_str("0"),
            ExtraCommits::Text(text) => f.write_str(text),
        }
    }
}

/// The dict `Machine._parseVersionString` returns:
/// `{"Release": Version, "ExtraCommits": ..., "SHA": str, "Local": str}`.
///
/// Equality is the dict's, which `should_start_firmware_update` uses to
/// decide whether to flash. PARITY: `ExtraCommits` is the int 0 without a
/// `-` part and a string otherwise, so `1.2.3` and `1.2.3-0` differ and
/// count as an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirmwareVersion {
    pub release: Pep440Version,
    pub extra_commits: ExtraCommits,
    pub sha: String,
    pub local: String,
}

/// `Machine._parseVersionString`: `git describe`-style text split on `-`
/// into release, extra commits, sha and modifier. `None` for `None`, the
/// empty string, or a release that is not PEP 440 (Python logs a warning).
///
/// When `should_start_firmware_update` is true, `_read_data` logs
/// `Firmware {Release}-{ExtraCommits} is outdated, upgrading` with
/// `str(Version)` and the extra commits as displayed here. PARITY: if the
/// running version did not parse, that log line raises `AttributeError` on
/// `None` and ends the serial reader thread before the update starts.
pub fn parse_version_string(version_str: Option<&str>) -> Option<FirmwareVersion> {
    let text = version_str.filter(|s| !s.is_empty())?;
    let mut components = py_strip(text).split('-');
    let release = Pep440Version::parse(components.next().unwrap_or_default()).ok()?;
    Some(FirmwareVersion {
        release,
        extra_commits: components
            .next()
            .map_or(ExtraCommits::Default, |c| ExtraCommits::Text(c.to_string())),
        sha: components.next().unwrap_or_default().to_string(),
        local: components.next().unwrap_or_default().to_string(),
    })
}

/// Where `get_version_from_firmware` raises instead of returning a value
/// (the exception reaches `Machine.init`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FirmwareImageError {
    /// The DROM segment is shorter than the 256-byte `esp_app_desc_t`.
    #[error("struct.error: the app description needs 256 bytes, got {0}")]
    ShortAppDescription(usize),
    /// A text field of the app description is not UTF-8.
    #[error("UnicodeDecodeError in the app description")]
    NotUtf8,
}

const ESP_IMAGE_MAGIC: u8 = 0xE9;
const APP_DESC_MAGIC: u32 = 0xABCD_5432;
const ESP32S3_DROM: std::ops::Range<u32> = 0x3C00_0000..0x3D00_0000;

fn le_u32(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
}

/// `ESPToolWrapper.get_version_from_firmware` on the bytes of
/// `esp32-s3/firmware.bin`: the image is read the way esptool's
/// `ESP32S3FirmwareImage` loads it (header, extended header, segments,
/// checksum), and the version is the `esp_app_desc_t` at the start of the
/// last DROM segment, stripped of spaces, NULs and tabs.
///
/// `Ok(None)` where Python returns `None` (an image esptool rejects, no
/// DROM segment, a wrong app description magic).
pub fn firmware_image_app_version(image: &[u8]) -> Result<Option<String>, FirmwareImageError> {
    let Some(app_desc) = drom_app_desc(image) else {
        return Ok(None);
    };
    if app_desc.is_empty() {
        return Ok(None);
    }
    if app_desc.len() < 256 {
        return Err(FirmwareImageError::ShortAppDescription(app_desc.len()));
    }
    if le_u32(app_desc) != Some(APP_DESC_MAGIC) {
        return Ok(None);
    }
    let text = |range: std::ops::Range<usize>| {
        std::str::from_utf8(app_desc.get(range).unwrap_or_default())
            .map_err(|_| FirmwareImageError::NotUtf8)
    };
    // Python decodes these for its log lines before returning the version.
    let version = text(16..48)?;
    for range in [48..80, 80..96, 96..112, 112..144] {
        text(range)?;
    }
    Ok(Some(
        version
            .trim_matches(|c| c == ' ' || c == '\0' || c == '\t')
            .to_string(),
    ))
}

/// The first 256 bytes of the last DROM segment; `None` where esptool's
/// loader raises.
fn drom_app_desc(image: &[u8]) -> Option<&[u8]> {
    let header = image.get(..8)?;
    if header[0] != ESP_IMAGE_MAGIC {
        return None;
    }
    let segment_count = header[1];
    // The last extended header byte says whether a SHA-256 digest follows
    // the checksum; esptool rejects anything but 0 or 1 and reads the
    // digest without checking its length.
    let extended = image.get(8..24)?;
    if extended[15] > 1 {
        return None;
    }
    let mut pos = 24;
    let mut app_desc = None;
    for _ in 0..segment_count {
        let addr = le_u32(image.get(pos..pos + 4)?)?;
        let size = le_u32(image.get(pos + 4..pos + 8)?)? as usize;
        let data = image.get(pos + 8..pos.checked_add(8 + size)?)?;
        pos += 8 + size;
        if ESP32S3_DROM.contains(&addr) {
            app_desc = Some(&data[..data.len().min(256)]);
        }
    }
    // The checksum byte sits at the end of the next 16-byte block.
    pos += 15 - pos % 16;
    image.get(pos)?;
    if segment_count > 16 {
        return None;
    }
    Some(app_desc.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Pep440Version {
        Pep440Version::parse(text).unwrap()
    }

    /// Expected values from `packaging.version.Version` 24.1.
    #[test]
    fn pep440_normalization_and_order() {
        assert_eq!(v("1.0RC1").to_string(), "1.0rc1");
        assert_eq!(v("v1!2.0.post.dev").to_string(), "1!2.0.post0.dev0");
        assert_eq!(v("1.0+Local_01-x").to_string(), "1.0+local.1.x");
        assert_eq!(v("01.002").to_string(), "1.2");
        assert!(v("1.0.dev0") < v("1.0a0"));
        assert!(v("1.0a0") < v("1.0"));
        assert!(v("1.0") < v("1.0.post0"));
        assert!(v("1.0+5") > v("1.0+abc"));
        assert_eq!(v("1.0"), v("1.0.0"));
        assert!(Pep440Version::parse("abc").is_err());
    }

    /// `Machine._parseVersionString` on the formats the firmware reports.
    #[test]
    fn firmware_version_components() {
        let parsed = parse_version_string(Some("0.2.24-251-gd519a85")).unwrap();
        assert_eq!(parsed.release.to_string(), "0.2.24");
        assert_eq!(parsed.extra_commits, ExtraCommits::Text("251".to_string()));
        assert_eq!(parsed.sha, "gd519a85");
        assert_eq!(parsed.local, "");
        assert_eq!(parse_version_string(Some("")), None);
        assert_eq!(parse_version_string(None), None);
        assert_eq!(parse_version_string(Some("dirty")), None);
        // PARITY: the int 0 is not the string "0".
        assert_ne!(
            parse_version_string(Some("1.2.3")),
            parse_version_string(Some("1.2.3-0"))
        );
    }
}
