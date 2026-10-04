//! The GitHub release ambit updates from: which version is latest, and the bytes of one asset.
//!
//! Nothing here calls the GitHub API. The latest tag is read from the redirect `/releases/latest`
//! already answers with, and the assets are fetched from the same public download URLs
//! `install.sh` uses. The API would need no token either, but it is rate-limited to 60 requests an
//! hour per address, which a shared office address or a CI runner can exhaust; a redirect and a
//! download are not.
//!
//! The download is streamed and hashed as it passes, so an archive never has to be held in memory
//! to be checked.
//!
//! HTTP goes through the [`Http`] trait, so a test supplies canned responses instead of reaching
//! GitHub. [`UreqHttp`] is the real implementation.

use std::cmp::Ordering;
use std::io::{Read, Write as _};
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use sha2::{Digest as _, Sha256};

use crate::errors::{AmbitError, Result, network_error};
use crate::util::hash::hex;
use crate::util::text::js_trim;

/// The repository releases are published from. Matches `REPO` in `install.sh`.
const REPO: &str = "aldesantis/ambit";

static RELEASES_URL: LazyLock<String> =
    LazyLock::new(|| format!("https://github.com/{REPO}/releases"));

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

impl HttpResponse {
    /// Whether the status is a 2xx, as `Response.ok` is.
    fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
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
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(options.timeout))
            // Zero returns the 3xx itself, which is how `latest_tag` reads the tag.
            .max_redirects(if options.follow_redirects { 10 } else { 0 })
            .http_status_as_error(false)
            .build()
            .into();
        let response = agent.get(url).call().map_err(|error| error.to_string())?;
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        Ok(HttpResponse {
            status: response.status().as_u16(),
            location,
            body: Box::new(response.into_body().into_reader()),
        })
    }
}

/// One line of a `.sha256` file: the hash, then the file name, which `sha256sum` prefixes with `*`
/// in binary mode.
static CHECKSUM_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9a-f]{64})(?:\s+\*?(\S+))?$").expect("a valid pattern"));

static TAG_IN_LOCATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/releases/tag/([^/?#]+)").expect("a valid pattern"));

/// Where one file of one release is downloaded from.
pub fn asset_url(tag: &str, asset: &str) -> String {
    format!("{}/download/{tag}/{asset}", *RELEASES_URL)
}

/// The tag for a version a user typed, which may or may not carry the `v` a tag has.
pub fn as_tag(version: &str) -> String {
    if version.starts_with('v') {
        version.to_owned()
    } else {
        format!("v{version}")
    }
}

/// A version, or `None` for anything that is not one. Callers never guess at an ordering.
///
/// A tag carries a leading `v` and `Cargo.toml` does not, so either spelling is accepted.
fn parse_version(text: &str) -> Option<semver::Version> {
    let text = js_trim(text);

    semver::Version::parse(text.strip_prefix('v').unwrap_or(text)).ok()
}

/// Whether `candidate` is a release worth moving to from `current`. Unparseable means no.
///
/// Compared by semver precedence, so a prerelease sorts below the release it leads to and build
/// metadata orders nothing: `1.0.0-rc.1` never counts as an update for someone already on `1.0.0`.
pub fn is_newer(current: &str, candidate: &str) -> bool {
    match (parse_version(current), parse_version(candidate)) {
        (Some(from), Some(to)) => to.cmp_precedence(&from) == Ordering::Greater,
        _ => false,
    }
}

/// `decodeURIComponent`, falling back to the text as given when it is not valid percent-encoded
/// UTF-8.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(byte) = text
                .get(index + 1..index + 3)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        {
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(decoded).unwrap_or_else(|_| text.to_owned())
}

/// The tag of the newest published release.
///
/// `/releases/latest` answers with a redirect to `/releases/tag/<tag>`, which is where the tag is
/// read from. Not following the redirect is what keeps this one request rather than one request
/// and a download of the release page's HTML.
///
/// # Errors
///
/// Exit 4 when the request fails, or when the redirect names no tag, which is what a repository
/// with no published release answers.
pub fn latest_tag(http: &dyn Http, timeout: Duration) -> Result<String> {
    let url = format!("{}/latest", *RELEASES_URL);
    let response = http
        .get(
            &url,
            &GetOptions {
                timeout,
                follow_redirects: false,
            },
        )
        .map_err(|error| {
            network_error(
                "could not reach GitHub to find the latest ambit release",
                [
                    error,
                    "check the connection, or install a specific release with `ambit self-update <version>`"
                        .to_owned(),
                ],
            )
        })?;

    let location = response.location.clone().unwrap_or_default();
    let Some(tag) = TAG_IN_LOCATION.captures(&location).map(|c| c[1].to_owned()) else {
        return Err(network_error(
            "GitHub did not name a latest ambit release",
            [
                format!(
                    "{url} answered {} pointing at \"{location}\"",
                    response.status
                ),
                "this is what a repository with no published release answers".to_owned(),
            ],
        ));
    };

    Ok(percent_decode(&tag))
}

/// The error for a response that is not the asset.
fn not_attached(url: &str, status: u16, tag: &str, asset: &str) -> AmbitError {
    network_error(
        format!("could not download {asset}"),
        [
            format!("{url} answered {status}"),
            format!("check that release {tag} exists and attaches {asset}"),
        ],
    )
}

/// The error for a request that did not complete, or a body that could not be read or written.
fn transport_failed(url: &str, asset: &str, error: impl std::fmt::Display) -> AmbitError {
    network_error(
        format!("could not download {asset}"),
        [error.to_string(), format!("it was requested from {url}")],
    )
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
    let url = asset_url(tag, asset);
    let mut response = http
        .get(
            &url,
            &GetOptions {
                timeout,
                follow_redirects: true,
            },
        )
        .map_err(|error| transport_failed(&url, asset, error))?;

    if !response.ok() {
        return Err(not_attached(&url, response.status, tag, asset));
    }

    let mut bytes = Vec::new();
    response
        .body
        .read_to_end(&mut bytes)
        .map_err(|error| transport_failed(&url, asset, error))?;

    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The recorded hash for one asset, out of a `.sha256` body (`<hex>  <name>`, the name optionally
/// prefixed with `*`).
///
/// A line holding the hash alone is also accepted: the file is per asset, so it can only be this
/// asset's hash.
///
/// # Errors
///
/// Exit 4 when the body names no line for that asset, which would otherwise leave the download
/// unverified.
pub fn checksum_for(checksums: &str, asset: &str) -> Result<String> {
    for line in checksums.split('\n') {
        if let Some(captures) = CHECKSUM_LINE.captures(js_trim(line))
            && captures.get(2).is_none_or(|name| name.as_str() == asset)
        {
            return Ok(captures[1].to_owned());
        }
    }

    Err(network_error(
        format!("{asset}{CHECKSUM_SUFFIX} lists no entry for {asset}"),
        [
            "the release is incomplete, so the download cannot be verified",
            "report it at https://github.com/aldesantis/ambit/issues",
        ],
    ))
}

/// Creates `destination` for writing, executable where the platform has the bit.
fn create_executable(destination: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        options.mode(0o755);
    }

    options.open(destination)
}

/// Streams one asset to `destination` and returns its sha256, lowercase hex.
///
/// The hash is taken from the same bytes that reach the disk rather than from a re-read of the
/// file, so nothing that happens to the file afterwards can pass a check the download failed.
///
/// # Errors
///
/// Exit 4 when the request fails or the response is not the asset.
pub fn download_asset(
    http: &dyn Http,
    tag: &str,
    asset: &str,
    destination: &Path,
    timeout: Duration,
) -> Result<String> {
    let url = asset_url(tag, asset);
    let mut response = http
        .get(
            &url,
            &GetOptions {
                timeout,
                follow_redirects: true,
            },
        )
        .map_err(|error| transport_failed(&url, asset, error))?;

    if !response.ok() {
        return Err(not_attached(&url, response.status, tag, asset));
    }

    let mut hash = Sha256::new();
    // The executable bit is set here rather than after the swap: the file has to be runnable
    // before it takes the place of one that is.
    let copied = (|| -> std::io::Result<()> {
        let mut file = create_executable(destination)?;
        let mut buffer = vec![0; 64 * 1024];

        loop {
            let read = response.body.read(&mut buffer)?;

            if read == 0 {
                break;
            }

            hash.update(&buffer[..read]);
            file.write_all(&buffer[..read])?;
        }

        file.flush()
    })();

    copied.map_err(|error| transport_failed(&url, asset, error))?;

    Ok(hex(&hash.finalize()))
}

#[cfg(test)]
mod tests;
