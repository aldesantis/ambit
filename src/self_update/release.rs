//! Avoids the GitHub API on purpose: its 60 requests/hour/address limit is exhausted by
//! shared offices and CI runners.

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

/// Matches `REPO` in `install.sh`.
const REPO: &str = "aldesantis/ambit";

static RELEASES_URL: LazyLock<String> =
    LazyLock::new(|| format!("https://github.com/{REPO}/releases"));

pub const CHECKSUM_SUFFIX: &str = ".sha256";

pub const METADATA_TIMEOUT: Duration = Duration::from_secs(10);

pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetOptions {
    pub timeout: Duration,
    pub follow_redirects: bool,
}

pub struct HttpResponse {
    pub status: u16,
    pub location: Option<String>,
    pub body: Box<dyn Read>,
}

impl HttpResponse {
    fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

pub trait Http {
    fn get(&self, url: &str, options: &GetOptions) -> std::result::Result<HttpResponse, String>;
}

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

static CHECKSUM_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9a-f]{64})(?:\s+\*?(\S+))?$").expect("a valid pattern"));

static TAG_IN_LOCATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/releases/tag/([^/?#]+)").expect("a valid pattern"));

pub fn asset_url(tag: &str, asset: &str) -> String {
    format!("{}/download/{tag}/{asset}", *RELEASES_URL)
}

pub fn as_tag(version: &str) -> String {
    if version.starts_with('v') {
        version.to_owned()
    } else {
        format!("v{version}")
    }
}

fn parse_version(text: &str) -> Option<semver::Version> {
    let text = js_trim(text);

    semver::Version::parse(text.strip_prefix('v').unwrap_or(text)).ok()
}

pub fn is_newer(current: &str, candidate: &str) -> bool {
    match (parse_version(current), parse_version(candidate)) {
        (Some(from), Some(to)) => to.cmp_precedence(&from) == Ordering::Greater,
        _ => false,
    }
}

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

fn not_attached(url: &str, status: u16, tag: &str, asset: &str) -> AmbitError {
    network_error(
        format!("could not download {asset}"),
        [
            format!("{url} answered {status}"),
            format!("check that release {tag} exists and attaches {asset}"),
        ],
    )
}

fn transport_failed(url: &str, asset: &str, error: impl std::fmt::Display) -> AmbitError {
    network_error(
        format!("could not download {asset}"),
        [error.to_string(), format!("it was requested from {url}")],
    )
}

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
    // Set the executable bit before the swap, never after it.
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
