//! The herdr this build is pinned to, as the running app reads it.
//!
//! `deps/herdr.pin` is what `./dev` downloads from and verifies against, and until now that was
//! the only thing that read it - so "the pin decides the version" was true of the suite and not
//! of the app. Putting a daemon on another machine makes the app an acquirer too, and it has to
//! acquire the same bytes for the same reason: a daemon the corpus was never recorded against
//! is a window whose every behaviour is unverified.
//!
//! Compiled in rather than shipped beside the binary, the way `muster-cli` embeds its
//! documentation. A pin a person could edit after the build would be a version claim nothing
//! stands behind, and there is nowhere in an app bundle a file like this belongs.

use std::collections::BTreeMap;

/// What this build was pinned to, verbatim.
const PIN: &str = include_str!("../../../deps/herdr.pin");

/// A version, and the exact bytes of each platform's release asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub version: String,
    /// Keyed by asset name - `herdr-linux-aarch64` and its three siblings.
    pub checksums: BTreeMap<String, String>,
}

impl Pin {
    /// The sha256 of one platform's asset, or nothing if the pin does not carry that platform.
    pub fn checksum(&self, asset: &str) -> Option<&str> {
        self.checksums.get(asset).map(String::as_str)
    }

    /// Where this asset is published.
    ///
    /// herdr has no manifest, so this is the release URL rebuilt from the version - the same
    /// shape `./dev` uses, and the reason the checksums in the pin are ours rather than
    /// upstream's.
    pub fn url(&self, asset: &str) -> String {
        format!("https://github.com/herdrdev/herdr/releases/download/v{}/{asset}", self.version)
    }
}

/// Reads the pin this build carries.
///
/// A failure here is this repo's own file being malformed, which `./dev` refuses before
/// anything is built - so it is a bug rather than a state to design around. It is still a
/// `Result` rather than a panic, because the one caller is starting a daemon on somebody's
/// devenv and a refusal naming the reason is worth more there than a crashed window.
pub fn pinned() -> Result<Pin, String> {
    let parsed: serde_json::Value = serde_json::from_str(PIN)
        .map_err(|error| format!("Muster's own herdr pin is not valid JSON ({error})"))?;
    let version = parsed
        .get("version")
        .and_then(serde_json::Value::as_str)
        .ok_or("Muster's own herdr pin names no version")?
        .to_string();
    let checksums = parsed
        .get("checksums")
        .and_then(serde_json::Value::as_object)
        .ok_or("Muster's own herdr pin carries no checksums")?
        .iter()
        .filter_map(|(asset, sum)| Some((asset.clone(), sum.as_str()?.to_string())))
        .collect();
    Ok(Pin { version, checksums })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pin is compiled in, so a malformed one is a broken build rather than a broken run.
    /// This is what turns that into a failing test instead of a devenv that will not attach.
    #[test]
    fn the_pin_this_build_carries_parses() {
        let pin = pinned().expect("the pin in deps/ should parse");
        assert!(!pin.version.is_empty(), "the pin should name a version");
        assert_eq!(
            pin.checksums.len(),
            4,
            "the pin should carry all four platforms, and carries {:?}",
            pin.checksums.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn every_checksum_is_a_sha256() {
        let pin = pinned().expect("the pin in deps/ should parse");
        for (asset, sum) in &pin.checksums {
            assert_eq!(sum.len(), 64, "{asset} should carry a sha256, and carries {sum:?}");
            assert!(
                sum.chars().all(|digit| digit.is_ascii_hexdigit()),
                "{asset} should carry hex, and carries {sum:?}"
            );
        }
    }

    #[test]
    fn an_asset_url_names_the_pinned_release() {
        let pin = pinned().expect("the pin in deps/ should parse");
        let url = pin.url("herdr-linux-aarch64");
        assert!(
            url.ends_with(&format!("/v{}/herdr-linux-aarch64", pin.version)),
            "the url should name the pinned version, and is {url}"
        );
    }
}
