//! Published releases: versions, the latest tag, and downloading a verified asset.
//!
//! HTTP goes through the [`Http`] trait, so a test supplies canned responses instead of reaching
//! GitHub. [`UreqHttp`] is the real implementation.

use std::cmp::Ordering;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use crate::errors::Result;

/// The suffix of the file cargo-dist attaches beside each asset: one `sha256sum` line for it.
pub const CHECKSUM_SUFFIX: &str = ".sha256";

/// How long a metadata request may take. Short: it is one redirect or a few hundred bytes.
pub const METADATA_TIMEOUT: Duration = Duration::from_secs(10);

/// How long an asset download may take. Long: it is an executable on an unknown connection.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// How one GET is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetOptions {
    /// The whole request, body included, must finish within this.
    pub timeout: Duration,
    /// Whether to follow redirects. `false` returns a 3xx as the response, with `location` set.
    pub follow_redirects: bool,
}

/// One HTTP response. Any status is a response, not an error.
pub struct HttpResponse {
    pub status: u16,
    /// The `Location` header, when present.
    pub location: Option<String>,
    /// The body, streamed.
    pub body: Box<dyn Read>,
}

/// The subset of HTTP this module uses, so a test can supply its own.
pub trait Http {
    /// GETs `url`.
    ///
    /// # Errors
    ///
    /// A message describing a request that did not complete (DNS, TLS, connection, timeout). An
    /// HTTP error status is not an error.
    fn get(&self, url: &str, options: &GetOptions) -> std::result::Result<HttpResponse, String>;
}

/// The real [`Http`]: ureq, blocking, rustls.
#[derive(Clone, Copy, Debug, Default)]
pub struct UreqHttp;

impl Http for UreqHttp {
    fn get(&self, url: &str, options: &GetOptions) -> std::result::Result<HttpResponse, String> {
        let _ = (url, options);
        todo!("port self/release.ts:fetch (ureq)")
    }
}

/// A parsed release version. Build metadata is not kept: it does not order two versions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// The dot-separated identifiers after `-`, empty for a normal release.
    pub prerelease: Vec<String>,
}

/// Where one file of one release is downloaded from.
pub fn asset_url(tag: &str, asset: &str) -> String {
    let _ = (tag, asset);
    todo!("port self/release.ts:assetUrl")
}

/// The tag for a version a user typed, which may or may not carry the `v` a tag has.
pub fn as_tag(version: &str) -> String {
    let _ = version;
    todo!("port self/release.ts:asTag")
}

/// A version, or `None` for anything that is not one. Callers never guess at an ordering.
pub fn parse_version(text: &str) -> Option<Version> {
    let _ = text;
    todo!("port self/release.ts:parseVersion")
}

/// How `a` orders against `b`: `Less` when `a` is older.
///
/// A prerelease sorts below the release it leads to, so `1.0.0-rc.1` never counts as an update for
/// someone already on `1.0.0`.
pub fn compare_versions(a: &Version, b: &Version) -> Ordering {
    let _ = (a, b);
    todo!("port self/release.ts:compareVersions")
}

/// Whether `candidate` is a release worth moving to from `current`. Unparseable means no.
pub fn is_newer(current: &str, candidate: &str) -> bool {
    let _ = (current, candidate);
    todo!("port self/release.ts:isNewer")
}

/// The tag of the newest published release.
///
/// `/releases/latest` answers with a redirect to `/releases/tag/<tag>`, which is where the tag is
/// read from. Not following the redirect is what keeps this one request.
///
/// # Errors
///
/// Exit 4 when the request fails, or when the redirect names no tag, which is what a repository
/// with no published release answers.
pub fn latest_tag(http: &dyn Http, timeout: Duration) -> Result<String> {
    let _ = (http, timeout);
    todo!("port self/release.ts:latestTag")
}

/// The body of one release asset as text, for its `.sha256` file.
///
/// # Errors
///
/// Exit 4 when the request fails or the asset is not part of the release.
pub fn fetch_asset_text(
    http: &dyn Http,
    tag: &str,
    asset: &str,
    timeout: Duration,
) -> Result<String> {
    let _ = (http, tag, asset, timeout);
    todo!("port self/release.ts:fetchAssetText")
}

/// The recorded hash for one asset, out of a `.sha256` body (`<hex>  <name>`, the name optionally
/// prefixed with `*`).
///
/// # Errors
///
/// Exit 4 when the body names no line for that asset, which would otherwise leave the download
/// unverified.
pub fn checksum_for(checksums: &str, asset: &str) -> Result<String> {
    let _ = (checksums, asset);
    todo!("port self/release.ts:checksumFor")
}

/// Streams one asset to `destination` and returns its sha256, lowercase hex.
///
/// The hash is taken from the same bytes that reach the disk rather than from a re-read of the
/// file, so nothing that happens to the file afterwards can pass a check the download failed.
///
/// # Errors
///
/// Exit 4 when the request fails or the response carries no body.
pub fn download_asset(
    http: &dyn Http,
    tag: &str,
    asset: &str,
    destination: &Path,
    timeout: Duration,
) -> Result<String> {
    let _ = (http, tag, asset, destination, timeout);
    todo!("port self/release.ts:downloadAsset")
}
