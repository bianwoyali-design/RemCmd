//! Official-release discovery and bounded, cancellable, verified downloads.
use reqwest::{Client, StatusCode, Url};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::watch,
};

pub const RELEASES_URL: &str = "https://github.com/bianwoyali-design/RemCmd/releases";
const LATEST_URL: &str = "https://api.github.com/repos/bianwoyali-design/RemCmd/releases/latest";
const MAX_METADATA_BYTES: usize = 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug)]
pub enum UpdateError {
    Network(String),
    Http(u16),
    InvalidRelease,
    UnsafeUrl,
    MissingDigest,
    TooLarge,
    Integrity,
    Cancelled,
    Io(std::io::Error),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(error) => write!(f, "{error}"),
            Self::Http(status) => write!(f, "GitHub returned HTTP {status}"),
            Self::InvalidRelease => f.write_str("Invalid release metadata"),
            Self::UnsafeUrl => f.write_str("Unexpected download URL"),
            Self::MissingDigest => f.write_str("The package has no SHA-256 checksum"),
            Self::TooLarge => f.write_str("The response exceeds the allowed size"),
            Self::Integrity => f.write_str("Package size or checksum does not match"),
            Self::Cancelled => f.write_str("Download cancelled"),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}
impl std::error::Error for UpdateError {}
impl From<std::io::Error> for UpdateError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<reqwest::Error> for UpdateError {
    fn from(error: reqwest::Error) -> Self {
        Self::Network(error.without_url().to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageKind {
    MacArm,
    MacIntel,
    Windows,
    Deb,
    AppImage,
}
impl PackageKind {
    pub fn for_platform(os: &str, arch: &str, appimage: bool, debian: bool) -> Option<Self> {
        match (os, arch) {
            ("macos", "aarch64") => Some(Self::MacArm),
            ("macos", "x86_64") => Some(Self::MacIntel),
            ("windows", "x86_64") => Some(Self::Windows),
            ("linux", "x86_64") if debian && !appimage => Some(Self::Deb),
            ("linux", "x86_64") => Some(Self::AppImage),
            _ => None,
        }
    }
    fn suffix(self) -> &'static str {
        match self {
            Self::MacArm => "macos-aarch64.dmg",
            Self::MacIntel => "macos-x86_64.dmg",
            Self::Windows => "windows-x86_64.msi",
            Self::Deb => "linux-x86_64.deb",
            Self::AppImage => "linux-x86_64.AppImage",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub size: u64,
    url: String,
    sha256: String,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub notes: String,
    pub package: Option<Package>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DownloadProgress {
    pub received: u64,
    pub total: u64,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    body: Option<String>,
    assets: Vec<ApiAsset>,
}
#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    size: u64,
    browser_download_url: String,
    digest: Option<String>,
    state: String,
}

pub fn parse_release(
    bytes: &[u8],
    current: &str,
    platform: Option<PackageKind>,
) -> Result<Option<Release>, UpdateError> {
    if bytes.len() > MAX_METADATA_BYTES {
        return Err(UpdateError::TooLarge);
    }
    let release: ApiRelease =
        serde_json::from_slice(bytes).map_err(|_| UpdateError::InvalidRelease)?;
    let version = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .ok_or(UpdateError::InvalidRelease)?,
    )
    .map_err(|_| UpdateError::InvalidRelease)?;
    let current = Version::parse(current).map_err(|_| UpdateError::InvalidRelease)?;
    if release.draft
        || release.prerelease
        || !version.pre.is_empty()
        || !version.cmp_precedence(&current).is_gt()
    {
        return Ok(None);
    }
    let name = platform.map(|kind| format!("RemCmd-v{version}-{}", kind.suffix()));
    let package = release
        .assets
        .into_iter()
        .find(|asset| Some(&asset.name) == name.as_ref() && asset.state == "uploaded")
        .map(|asset| {
            if asset.size == 0 || asset.size > MAX_PACKAGE_BYTES {
                return Err(UpdateError::TooLarge);
            }
            let expected_url = format!(
                "{RELEASES_URL}/download/{}/{}",
                release.tag_name, asset.name
            );
            if asset.browser_download_url != expected_url {
                return Err(UpdateError::UnsafeUrl);
            }
            let digest = asset
                .digest
                .and_then(|digest| digest.strip_prefix("sha256:").map(str::to_owned))
                .filter(|hash| {
                    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                .ok_or(UpdateError::MissingDigest)?;
            Ok(Package {
                name: asset.name,
                size: asset.size,
                url: expected_url,
                sha256: digest,
            })
        })
        .transpose()?;
    Ok(Some(Release {
        version: version.to_string(),
        url: format!("{RELEASES_URL}/tag/{}", release.tag_name),
        notes: release
            .body
            .unwrap_or_default()
            .chars()
            .take(4000)
            .collect(),
        package,
    }))
}

fn trusted_redirect(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none_or(|port| port == 443)
        && matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "api.github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
}

fn client() -> Result<Client, UpdateError> {
    Ok(Client::builder()
        .user_agent(concat!("RemCmd/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 || !trusted_redirect(attempt.url()) {
                attempt.error("Untrusted release redirect")
            } else {
                attempt.follow()
            }
        }))
        .build()?)
}

pub async fn check(
    current: &str,
    platform: Option<PackageKind>,
) -> Result<Option<Release>, UpdateError> {
    let mut response = client()?
        .get(LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .timeout(Duration::from_secs(20))
        .send()
        .await?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(UpdateError::Http(response.status().as_u16()));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_METADATA_BYTES {
            return Err(UpdateError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_release(&bytes, current, platform)
}

async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            return;
        }
    }
}

pub async fn download(
    package: &Package,
    directory: &Path,
    mut cancel: watch::Receiver<bool>,
    progress: watch::Sender<DownloadProgress>,
) -> Result<PathBuf, UpdateError> {
    tokio::select! {
        biased;
        _ = cancelled(&mut cancel) => Err(UpdateError::Cancelled),
        result = download_inner(package, directory, progress) => result,
    }
}

async fn download_inner(
    package: &Package,
    directory: &Path,
    progress: watch::Sender<DownloadProgress>,
) -> Result<PathBuf, UpdateError> {
    if package.size == 0
        || package.size > MAX_PACKAGE_BYTES
        || !package
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
        || !package.name.starts_with("RemCmd-")
    {
        return Err(UpdateError::InvalidRelease);
    }
    let response = client()?
        .get(&package.url)
        .timeout(Duration::from_secs(1800))
        .send()
        .await?;
    receive_package(package, directory, progress, response).await
}

async fn receive_package(
    package: &Package,
    directory: &Path,
    progress: watch::Sender<DownloadProgress>,
    mut response: reqwest::Response,
) -> Result<PathBuf, UpdateError> {
    if !response.status().is_success() {
        return Err(UpdateError::Http(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|size| size != package.size)
    {
        return Err(UpdateError::Integrity);
    }
    tokio::fs::create_dir_all(directory).await?;
    let temporary = tempfile::NamedTempFile::new_in(directory)?;
    let mut output = tokio::fs::File::from_std(temporary.reopen()?);
    let mut hasher = Sha256::new();
    let mut received = 0;
    while let Some(chunk) = response.chunk().await? {
        received += chunk.len() as u64;
        if received > package.size || received > MAX_PACKAGE_BYTES {
            return Err(UpdateError::TooLarge);
        }
        hasher.update(&chunk);
        output.write_all(&chunk).await?;
        progress.send_replace(DownloadProgress {
            received,
            total: package.size,
        });
    }
    if received != package.size
        || !format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(&package.sha256)
    {
        return Err(UpdateError::Integrity);
    }
    output.flush().await?;
    output.sync_all().await?;
    drop(output);
    let path = directory.join(&package.name);
    temporary
        .persist(&path)
        .map_err(|error| UpdateError::Io(error.error))?;
    Ok(path)
}

/// Verify again immediately before opening a previously downloaded package.
pub async fn verify(package: &Package, path: &Path) -> Result<(), UpdateError> {
    let mut file = tokio::fs::File::open(path).await?;
    if file.metadata().await?.len() != package.size {
        return Err(UpdateError::Integrity);
    }
    let mut buffer = [0; 64 * 1024];
    let mut hasher = Sha256::new();
    let mut received = 0;
    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        received += count as u64;
        if received > package.size {
            return Err(UpdateError::Integrity);
        }
        hasher.update(&buffer[..count]);
    }
    if received != package.size
        || !format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(&package.sha256)
    {
        return Err(UpdateError::Integrity);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> serde_json::Value {
        serde_json::json!({"tag_name":tag,"draft":false,"prerelease":false,"body":"Release notes", "assets":[{"name":format!("RemCmd-{tag}-macos-aarch64.dmg"),"size":3,"state":"uploaded", "browser_download_url":format!("{RELEASES_URL}/download/{tag}/RemCmd-{tag}-macos-aarch64.dmg"),"digest":format!("sha256:{:x}",Sha256::digest(b"abc"))}]})
    }
    fn parse(value: &serde_json::Value, current: &str) -> Result<Option<Release>, UpdateError> {
        parse_release(
            &serde_json::to_vec(value).unwrap(),
            current,
            Some(PackageKind::MacArm),
        )
    }

    #[test]
    fn stable_updates_compare_semver_precedence_without_build_metadata() {
        assert!(parse(&release("v0.1.0"), "0.1.0-rc.1").unwrap().is_some());
        assert!(parse(&release("v0.1.0"), "0.1.0").unwrap().is_none());
        assert!(parse(&release("v0.1.0"), "0.2.0").unwrap().is_none());
        assert!(
            parse(&release("v0.1.0+new"), "0.1.0+old")
                .unwrap()
                .is_none()
        );
        for key in ["draft", "prerelease"] {
            let mut value = release("v0.2.0");
            value[key] = true.into();
            assert!(parse(&value, "0.1.0").unwrap().is_none());
        }
        assert!(parse(&release("v0.2.0-beta.1"), "0.1.0").unwrap().is_none());
    }

    #[test]
    fn packages_must_match_platform_source_and_digest() {
        let mut value = release("v0.2.0");
        assert!(parse(&value, "0.1.0").unwrap().unwrap().package.is_some());
        assert!(
            parse_release(
                &serde_json::to_vec(&value).unwrap(),
                "0.1.0",
                Some(PackageKind::Windows)
            )
            .unwrap()
            .unwrap()
            .package
            .is_none()
        );
        value["assets"][0]["browser_download_url"] = "https://example.com/installer".into();
        assert!(matches!(
            parse(&value, "0.1.0"),
            Err(UpdateError::UnsafeUrl)
        ));
        value = release("v0.2.0");
        value["assets"][0]["digest"] = serde_json::Value::Null;
        assert!(matches!(
            parse(&value, "0.1.0"),
            Err(UpdateError::MissingDigest)
        ));
        value["assets"][0]["size"] = (MAX_PACKAGE_BYTES + 1).into();
        assert!(matches!(parse(&value, "0.1.0"), Err(UpdateError::TooLarge)));
    }

    #[test]
    fn platform_selection_preserves_appimage_installs_and_rejects_other_architectures() {
        assert_eq!(
            PackageKind::for_platform("linux", "x86_64", true, true),
            Some(PackageKind::AppImage)
        );
        assert_eq!(
            PackageKind::for_platform("linux", "x86_64", false, true),
            Some(PackageKind::Deb)
        );
        assert_eq!(
            PackageKind::for_platform("macos", "aarch64", false, false),
            Some(PackageKind::MacArm)
        );
        assert_eq!(
            PackageKind::for_platform("windows", "aarch64", false, false),
            None
        );
    }

    #[test]
    fn redirects_cannot_downgrade_tls_or_leave_github() {
        assert!(trusted_redirect(
            &Url::parse("https://release-assets.githubusercontent.com/file?token=temporary")
                .unwrap()
        ));
        for url in [
            "http://github.com/file",
            "https://github.com.evil.test/file",
            "https://user@github.com/file",
            "https://github.com:8443/file",
            "https://example.com/file",
        ] {
            assert!(!trusted_redirect(&Url::parse(url).unwrap()));
        }
    }

    #[tokio::test]
    async fn opening_a_cached_package_rechecks_size_and_contents() {
        let package = parse(&release("v0.2.0"), "0.1.0")
            .unwrap()
            .unwrap()
            .package
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(&package.name);
        tokio::fs::write(&path, b"abc").await.unwrap();
        verify(&package, &path).await.unwrap();
        tokio::fs::write(&path, b"xyz").await.unwrap();
        assert!(matches!(
            verify(&package, &path).await,
            Err(UpdateError::Integrity)
        ));
        tokio::fs::write(&path, b"shorter or longer").await.unwrap();
        assert!(matches!(
            verify(&package, &path).await,
            Err(UpdateError::Integrity)
        ));
    }

    #[tokio::test]
    async fn cancelled_download_does_not_create_a_file_or_send_a_request() {
        let package = parse(&release("v0.2.0"), "0.1.0")
            .unwrap()
            .unwrap()
            .package
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let (_cancel_tx, cancel) = watch::channel(true);
        let (progress, _) = watch::channel(DownloadProgress::default());
        assert!(matches!(
            download(&package, temp.path(), cancel, progress).await,
            Err(UpdateError::Cancelled)
        ));
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    async fn response(
        body: &'static [u8],
        declared_size: usize,
        hold: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> reqwest::Response {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                let count = socket.read(&mut request).await.unwrap();
                assert!(count > 0, "test request must include complete headers");
                headers.extend_from_slice(&request[..count]);
                assert!(headers.len() <= 8192, "test request headers are bounded");
            }
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {declared_size}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            socket.write_all(body).await.unwrap();
            if let Some(hold) = hold {
                let _ = hold.await;
            }
        });
        Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{address}/fixture"))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn streamed_packages_publish_only_after_size_and_hash_verification() {
        let package = parse(&release("v0.2.0"), "0.1.0")
            .unwrap()
            .unwrap()
            .package
            .unwrap();
        for (body, succeeds) in [(b"abc" as &'static [u8], true), (b"xyz", false)] {
            let directory = tempfile::tempdir().unwrap();
            let (progress, receiver) = watch::channel(DownloadProgress::default());
            let result = receive_package(
                &package,
                directory.path(),
                progress,
                response(body, 3, None).await,
            )
            .await;
            if succeeds {
                assert_eq!(tokio::fs::read(result.unwrap()).await.unwrap(), b"abc");
                assert_eq!(receiver.borrow().received, 3);
            } else {
                assert!(matches!(result, Err(UpdateError::Integrity)));
                assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
            }
        }
    }

    #[tokio::test]
    async fn cancellation_removes_an_incomplete_streamed_package() {
        let package = parse(&release("v0.2.0"), "0.1.0")
            .unwrap()
            .unwrap()
            .package
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().to_path_buf();
        let (hold, wait) = tokio::sync::oneshot::channel();
        let response = response(b"a", 3, Some(wait)).await;
        let (progress, mut receiver) = watch::channel(DownloadProgress::default());
        let task =
            tokio::spawn(async move { receive_package(&package, &path, progress, response).await });
        tokio::time::timeout(Duration::from_secs(5), receiver.changed())
            .await
            .unwrap()
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        drop(hold);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
