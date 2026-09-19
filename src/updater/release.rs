use std::time::Duration;

use semver::Version;
use serde::Deserialize;
use url::Url;

const RELEASE_API: &str = "https://api.github.com/repos/kekkodance/ralgruM/releases/latest";
pub(super) const MAX_ASSET_BYTES: u64 = 250 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct Release {
    pub(crate) version: Version,
    pub(crate) asset_url: Url,
    pub(crate) page_url: Url,
    pub(crate) size: u64,
    pub(crate) digest: [u8; 32],
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    size: u64,
    digest: Option<String>,
    browser_download_url: String,
}

pub(crate) fn current_version() -> Version {
    #[cfg(debug_assertions)]
    if let Ok(version) = std::env::var("RALGRUM_UPDATE_TEST_CURRENT_VERSION")
        && let Ok(parsed) = Version::parse(&version)
    {
        return parsed;
    }
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the package version must be SemVer")
}

pub(crate) async fn fetch_latest(client: &reqwest::Client) -> Result<Option<Release>, String> {
    #[cfg(debug_assertions)]
    if let Some(candidate) = super::test_fixture::candidate_path() {
        return tokio::task::spawn_blocking(move || fixture_release(&candidate))
            .await
            .map_err(|error| format!("Could not inspect the updater test fixture: {error}"))?
            .map(Some);
    }
    let response = client
        .get(RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| format!("Could not check GitHub releases: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "GitHub release check returned HTTP {}",
            response.status()
        ));
    }
    let body = response
        .json::<GithubRelease>()
        .await
        .map_err(|error| format!("Could not read the GitHub release: {error}"))?;
    parse_release(body, &current_version())
}

#[cfg(debug_assertions)]
fn fixture_release(candidate: &std::path::Path) -> Result<Release, String> {
    use sha2::{Digest as _, Sha256};
    let mut file = std::fs::File::open(candidate)
        .map_err(|error| format!("Could not open the updater test candidate: {error}"))?;
    let size = file.metadata().map_err(|error| error.to_string())?.len();
    if size == 0 || size > MAX_ASSET_BYTES {
        return Err("The updater test candidate has an invalid size".into());
    }
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash).map_err(|error| error.to_string())?;
    let mut version =
        Version::parse(env!("CARGO_PKG_VERSION")).expect("the package version must be SemVer");
    if version <= current_version() {
        version = current_version();
        version.patch += 1;
    }
    let tag = format!("v{version}");
    Ok(Release {
        version,
        asset_url: Url::parse(&format!(
            "https://github.com/kekkodance/ralgruM/releases/download/{tag}/ralgruM.exe"
        ))
        .map_err(|error| error.to_string())?,
        page_url: Url::parse(&format!(
            "https://github.com/kekkodance/ralgruM/releases/tag/{tag}"
        ))
        .map_err(|error| error.to_string())?,
        size,
        digest: hash.finalize().into(),
    })
}

fn parse_release(body: GithubRelease, current: &Version) -> Result<Option<Release>, String> {
    if body.draft || body.prerelease {
        return Ok(None);
    }
    let tag = &body.tag_name;
    let version = Version::parse(tag.strip_prefix('v').unwrap_or(tag))
        .map_err(|_| "The latest GitHub release tag is not a valid version".to_owned())?;
    if version <= *current {
        return Ok(None);
    }
    let asset = body
        .assets
        .iter()
        .find(|asset| asset.name == "ralgruM.exe")
        .ok_or_else(|| "The latest release has no ralgruM.exe asset".to_owned())?;
    if asset.size == 0 || asset.size > MAX_ASSET_BYTES {
        return Err("The release executable has an invalid size".into());
    }
    let digest = parse_digest(
        asset
            .digest
            .as_deref()
            .ok_or("The release executable has no SHA-256 digest")?,
    )?;
    let asset_url = Url::parse(&asset.browser_download_url)
        .map_err(|_| "The release executable has an invalid URL".to_owned())?;
    let expected = format!("/kekkodance/ralgruM/releases/download/{tag}/ralgruM.exe");
    if asset_url.scheme() != "https"
        || asset_url.host_str() != Some("github.com")
        || asset_url.path() != expected
        || asset_url.query().is_some()
        || asset_url.fragment().is_some()
    {
        return Err("The release executable URL is outside the expected repository".into());
    }
    let page_url = Url::parse(&format!(
        "https://github.com/kekkodance/ralgruM/releases/tag/{tag}"
    ))
    .map_err(|_| "The release page URL is invalid".to_owned())?;
    Ok(Some(Release {
        version,
        asset_url,
        page_url,
        size: asset.size,
        digest,
    }))
}

fn parse_digest(value: &str) -> Result<[u8; 32], String> {
    let hex = value
        .strip_prefix("sha256:")
        .ok_or("The release executable has no SHA-256 digest")?;
    if hex.len() != 64 {
        return Err("The release executable SHA-256 digest is malformed".into());
    }
    let mut result = [0; 32];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
            .map_err(|_| "The release executable SHA-256 digest is malformed")?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(tag: &str, url: &str, digest: Option<&str>) -> GithubRelease {
        GithubRelease {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
            assets: vec![GithubAsset {
                name: "ralgruM.exe".into(),
                size: 47_000_000,
                digest: digest.map(str::to_owned),
                browser_download_url: url.into(),
            }],
        }
    }

    #[test]
    fn accepts_only_newer_stable_releases_with_the_expected_asset() {
        let digest = format!("sha256:{}", "ab".repeat(32));
        let url = "https://github.com/kekkodance/ralgruM/releases/download/v0.6.0/ralgruM.exe";
        let current = Version::parse("0.5.0").unwrap();
        let release = parse_release(response("v0.6.0", url, Some(&digest)), &current)
            .unwrap()
            .unwrap();
        assert_eq!(release.version, Version::parse("0.6.0").unwrap());
        assert_eq!(release.digest, [0xab; 32]);
        assert!(
            parse_release(response("v0.5.0", url, Some(&digest)), &current)
                .unwrap()
                .is_none()
        );
        assert!(
            parse_release(response("v0.4.9", url, Some(&digest)), &current)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn refuses_missing_digest_and_foreign_download_url() {
        let current = Version::parse("0.5.0").unwrap();
        let url = "https://github.com/kekkodance/ralgruM/releases/download/v0.6.0/ralgruM.exe";
        assert!(parse_release(response("v0.6.0", url, None), &current).is_err());
        let digest = format!("sha256:{}", "ab".repeat(32));
        assert!(
            parse_release(
                response("v0.6.0", "https://example.com/ralgruM.exe", Some(&digest)),
                &current
            )
            .is_err()
        );
    }

    #[tokio::test]
    #[ignore = "downloads the current public release executable"]
    async fn live_github_release_download_matches_its_digest() {
        let client = reqwest::Client::builder()
            .user_agent("ralgruM-updater-test")
            .build()
            .unwrap();
        let response = client.get(RELEASE_API).send().await.unwrap();
        let release = parse_release(
            response.json::<GithubRelease>().await.unwrap(),
            &Version::parse("0.0.0").unwrap(),
        )
        .unwrap()
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("ralgruM.exe");
        let downloaded = crate::updater::download::download(
            &client,
            &release,
            &destination,
            &tokio_util::sync::CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::metadata(&downloaded.staged_exe).unwrap().len(),
            release.size
        );
    }
}
