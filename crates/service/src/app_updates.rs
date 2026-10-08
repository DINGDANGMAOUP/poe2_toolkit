//! Signed, full-package releases handed to Velopack after verification.
//! Distribution keys are bundled or explicitly configured; downloads never supply trust.
use crate::{
    Store,
    updates::{SignedRules, UpdateSource},
};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use ed25519_dalek::Signature;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::io::AsyncWriteExt;
use velopack::{UpdateCheck, UpdateManager, UpdateOptions, VelopackAsset, VelopackAssetFeed};

const METADATA_LIMIT: usize = 256 * 1024;
pub const PACKAGE_ID: &str = "POE2Toolkit";

/// The recovery link comes from bundled configuration, never remote metadata.
pub fn download_page() -> String {
    let distribution: serde_json::Value =
        serde_json::from_str(include_str!("../../../config/distribution.json"))
            .expect("bundled distribution configuration");
    format!(
        "https://github.com/{}/releases",
        distribution["repository"]
            .as_str()
            .expect("bundled repository")
    )
}

fn supports_installation() -> bool {
    UpdateManager::new(velopack::sources::NoneSource {}, None, None)
        .is_ok_and(|manager| manager.get_app_id() == PACKAGE_ID)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppRelease {
    pub protocol: u32,
    pub channel: String,
    pub sequence: u64,
    pub target: String,
    pub published_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub package_url: String,
    pub asset: VelopackAsset,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Status {
    pub configured: bool,
    pub installed: bool,
    pub pending_version: Option<String>,
    pub available_version: Option<String>,
}
pub fn signing_bytes(payload: &str) -> Vec<u8> {
    [
        b"poe2-toolkit/application/v1\0".as_slice(),
        payload.as_bytes(),
    ]
    .concat()
}
pub fn target() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}
pub fn verify(
    source: &UpdateSource,
    envelope: &SignedRules,
    now: DateTime<Utc>,
) -> Result<AppRelease> {
    source.validate()?;
    ensure!(
        envelope.payload.len() <= METADATA_LIMIT && envelope.signature_hex.len() == 128,
        "程序发行记录超限"
    );
    source
        .key()?
        .verify_strict(
            &signing_bytes(&envelope.payload),
            &Signature::from_slice(&hex::decode(&envelope.signature_hex)?)?,
        )
        .context("程序发行签名无效")?;
    let release: AppRelease = serde_json::from_str(&envelope.payload)?;
    ensure!(
        release.protocol == 1
            && release.sequence > 0
            && release.channel == source.channel
            && release.target == target(),
        "程序发行协议、通道或平台不匹配"
    );
    ensure!(
        release.published_at <= now
            && release.expires_at > now
            && release.published_at < release.expires_at,
        "程序发行记录尚未生效或已过期"
    );
    let asset = &release.asset;
    ensure!(
        asset.PackageId == PACKAGE_ID && asset.Type == "Full",
        "只接受本应用的完整升级包"
    );
    ensure!(
        asset.Size > 0 && asset.Size <= 2 * 1024 * 1024 * 1024,
        "程序包长度超限"
    );
    ensure!(
        asset.SHA256.len() == 64 && hex::decode(&asset.SHA256)?.len() == 32,
        "程序包必须提供 SHA-256"
    );
    ensure!(
        asset.FileName.ends_with(".nupkg")
            && asset.FileName.len() <= 200
            && asset
                .FileName
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".-_".contains(&c))
            && !asset.FileName.starts_with('.'),
        "程序包名称不安全"
    );
    semver::Version::parse(&asset.Version)?;
    validate_url(&release.package_url)?;
    Ok(release)
}
fn validate_url(value: &str) -> Result<()> {
    let url = reqwest::Url::parse(value)?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none(),
        "程序源必须使用无凭据 HTTPS 地址"
    );
    Ok(())
}
pub fn configure(store: &Store, source: UpdateSource) -> Result<()> {
    source.validate()?;
    store.set("app_update_source", &source)
}
fn source(store: &Store) -> Result<UpdateSource> {
    match store.get("app_update_source")? {
        Some(source) => Ok(source),
        None => default_source(),
    }
}
pub(crate) fn default_source() -> Result<UpdateSource> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Distribution {
        repository: String,
        channel: String,
        public_key_hex: String,
    }
    let distribution: Distribution =
        serde_json::from_str(include_str!("../../../config/distribution.json"))?;
    ensure!(
        matches!(
            target().as_str(),
            "windows-x86_64" | "macos-aarch64" | "macos-x86_64"
        ),
        "此平台尚无官方程序发行包"
    );
    let source = UpdateSource {
        feed_url: format!(
            "https://raw.githubusercontent.com/{}/updates/{}/{}.json",
            distribution.repository,
            distribution.channel,
            target()
        ),
        channel: distribution.channel,
        public_key_hex: distribution.public_key_hex,
    };
    source.validate()?;
    Ok(source)
}
fn directory(store: &Store, envelope: &SignedRules) -> PathBuf {
    store
        .root
        .join("updates")
        .join(poe2_core::digest(envelope.payload.as_bytes()))
}
fn verify_package(path: &Path, release: &AppRelease) -> Result<()> {
    ensure!(
        std::fs::metadata(path)?.len() == release.asset.Size,
        "程序包长度校验失败"
    );
    ensure!(
        poe2_formats::hash_file(path)?.eq_ignore_ascii_case(&release.asset.SHA256),
        "程序包 SHA-256 校验失败"
    );
    Ok(())
}
fn manager(store: &Store, envelope: &SignedRules, release: &AppRelease) -> Result<UpdateManager> {
    let dir = directory(store, envelope);
    verify_package(&dir.join(&release.asset.FileName), release)?;
    let feed = VelopackAssetFeed {
        Assets: vec![release.asset.clone()],
    };
    std::fs::write(
        dir.join(format!("releases.{}.json", release.channel)),
        serde_json::to_vec(&feed)?,
    )?;
    UpdateManager::new(
        velopack::sources::FileSource::new(dir),
        Some(UpdateOptions {
            AllowVersionDowngrade: false,
            ExplicitChannel: Some(release.channel.clone()),
            MaximumDeltasBeforeFallback: -1,
        }),
        None,
    )
    .context("当前不是 Velopack 安装包，请先安装正式安装包后升级")
}
pub fn status(store: &Store) -> Result<Status> {
    let source = source(store).ok();
    let configured = source.is_some();
    let version = |key: &str| -> Result<Option<String>> {
        Ok(match (&source, store.get::<SignedRules>(key)?) {
            (Some(source), Some(envelope)) => verify(source, &envelope, Utc::now())
                .ok()
                .filter(|r| newer(&r.asset.Version))
                .map(|r| r.asset.Version),
            _ => None,
        })
    };
    let pending_version = version("pending_app_update")?;
    let available_version = version("available_app_update")?;
    Ok(Status {
        configured,
        installed: supports_installation(),
        pending_version,
        available_version,
    })
}
fn newer(version: &str) -> bool {
    semver::Version::parse(version)
        .is_ok_and(|v| v > semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap())
}
fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .https_only(true)
        .referer(false)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if redirect_allowed(attempt.url(), attempt.previous()) {
                attempt.follow()
            } else {
                attempt.error("程序下载重定向不受信任或次数超限")
            }
        }))
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(600))
        .build()?)
}
fn redirect_allowed(next: &reqwest::Url, previous: &[reqwest::Url]) -> bool {
    if previous.len() >= 5 || validate_url(next.as_str()).is_err() {
        return false;
    }
    let Some(origin) = previous.first() else {
        return false;
    };
    next.origin() == origin.origin()
        || (origin.host_str() == Some("github.com")
            && origin.port_or_known_default() == Some(443)
            && next.port_or_known_default() == Some(443)
            && matches!(
                next.host_str(),
                Some("release-assets.githubusercontent.com" | "objects.githubusercontent.com")
            ))
}
fn accepted(store: &Store, envelope: &SignedRules, release: &AppRelease) -> Result<()> {
    verify_package(
        &directory(store, envelope).join(&release.asset.FileName),
        release,
    )?;
    store.accept_app_release(envelope, release)
}
pub async fn check(store: &Store, cancel: &AtomicBool) -> Result<String> {
    check_with_progress(store, cancel, |_| {}).await
}

/// An idle probe validates metadata but never downloads or marks a package installable.
pub async fn probe(store: &Store, cancel: &AtomicBool) -> Result<String> {
    let (envelope, release) = fetch_release(store, cancel).await?;
    remember_available(store, &envelope, &release)?;
    Ok(if newer(&release.asset.Version) {
        format!("发现新版本 {}", release.asset.Version)
    } else {
        format!("已是最新版本 {}", env!("CARGO_PKG_VERSION"))
    })
}
fn remember_available(store: &Store, envelope: &SignedRules, release: &AppRelease) -> Result<()> {
    if newer(&release.asset.Version) {
        store.set("available_app_update", envelope)
    } else {
        store.remove("available_app_update")
    }
}
async fn fetch_release(store: &Store, cancel: &AtomicBool) -> Result<(SignedRules, AppRelease)> {
    let source = source(store)?;
    source.validate()?;
    let client = client()?;
    let mut response = client
        .get(&source.feed_url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?
        .error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(!cancel.load(Ordering::Relaxed), "已取消程序更新");
        ensure!(
            bytes.len() + chunk.len() <= METADATA_LIMIT,
            "程序发行记录超限"
        );
        bytes.extend_from_slice(&chunk);
    }
    let envelope: SignedRules = serde_json::from_slice(&bytes)?;
    let release = verify(&source, &envelope, Utc::now())?;
    store.check_app_release(&envelope, &release)?;
    Ok((envelope, release))
}

pub async fn check_with_progress(
    store: &Store,
    cancel: &AtomicBool,
    mut progress: impl FnMut(crate::update_activity::UpdatePhase),
) -> Result<String> {
    use crate::update_activity::UpdatePhase;
    progress(UpdatePhase::Checking);
    let (envelope, release) = fetch_release(store, cancel).await?;
    remember_available(store, &envelope, &release)?;
    let client = client()?;
    if !newer(&release.asset.Version) {
        return Ok(format!("已是最新版本 {}", env!("CARGO_PKG_VERSION")));
    }
    // A loose executable has no installation for Velopack to replace. Keep the
    // verified availability, but don't download a package that it cannot apply.
    if !supports_installation() {
        return Ok(format!(
            "发现新版本 {}；当前目录缺少更新组件，请获取完整安装包或便携包",
            release.asset.Version
        ));
    }
    let dir = directory(store, &envelope);
    std::fs::create_dir_all(&dir)?;
    ensure!(
        fs2::available_space(&dir)? > release.asset.Size * 3 + 64 * 1024 * 1024,
        "程序升级暂存空间不足"
    );
    let path = dir.join(&release.asset.FileName);
    if verify_package(&path, &release).is_err() {
        progress(UpdatePhase::Downloading {
            received: 0,
            total: release.asset.Size,
        });
        let temporary = dir.join(format!("{}.partial", uuid::Uuid::new_v4()));
        let result = async {
            let mut output = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .await?;
            let mut response = client
                .get(&release.package_url)
                .send()
                .await?
                .error_for_status()?;
            let mut length = 0;
            let mut last_progress = std::time::Instant::now();
            while let Some(chunk) = response.chunk().await? {
                ensure!(!cancel.load(Ordering::Relaxed), "已取消程序下载");
                length += chunk.len() as u64;
                ensure!(length <= release.asset.Size, "程序包超过签名声明长度");
                output.write_all(&chunk).await?;
                if last_progress.elapsed() >= std::time::Duration::from_millis(100)
                    || length == release.asset.Size
                {
                    progress(UpdatePhase::Downloading {
                        received: length,
                        total: release.asset.Size,
                    });
                    last_progress = std::time::Instant::now();
                }
            }
            output.sync_all().await?;
            drop(output);
            progress(UpdatePhase::Verifying);
            verify_package(&temporary, &release)?;
            std::fs::rename(&temporary, &path)?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result?;
    }
    progress(UpdatePhase::Verifying);
    accepted(store, &envelope, &release)?;
    Ok(format!(
        "程序 {} 已下载并验签，等待重启安装",
        release.asset.Version
    ))
}
/// Offline distribution accepts the identical signed envelope and full package.
pub fn import(store: &Store, envelope_path: &Path, package: &Path) -> Result<String> {
    ensure!(
        std::fs::metadata(envelope_path)?.len() <= METADATA_LIMIT as u64,
        "发行记录超限"
    );
    let envelope: SignedRules = serde_json::from_slice(&std::fs::read(envelope_path)?)?;
    let release = verify(&source(store)?, &envelope, Utc::now())?;
    store.check_app_release(&envelope, &release)?;
    ensure!(newer(&release.asset.Version), "升级版本必须高于当前版本");
    verify_package(package, &release)?;
    let dir = directory(store, &envelope);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(&release.asset.FileName);
    std::fs::copy(package, &path)?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)?
        .sync_all()?;
    accepted(store, &envelope, &release)?;
    Ok(release.asset.Version)
}
pub fn install(store: &Store) -> Result<()> {
    let _gate = crate::maintenance::acquire(store)?;
    let operations = crate::transaction::reconcile(store)?;
    ensure!(
        operations.iter().all(|o| matches!(
            o.state,
            poe2_core::OperationState::Committed
                | poe2_core::OperationState::RolledBack
                | poe2_core::OperationState::FailedUnchanged
        )),
        "存在待恢复操作，请先恢复再升级程序"
    );
    let envelope: SignedRules = store
        .get("pending_app_update")?
        .context("没有待安装的已验签程序包")?;
    let release = verify(&source(store)?, &envelope, Utc::now())?;
    store.check_app_release(&envelope, &release)?;
    ensure!(
        newer(&release.asset.Version),
        "程序版本已变化，请重新检查更新"
    );
    let manager = manager(store, &envelope, &release)?;
    ensure!(manager.get_app_id() == PACKAGE_ID, "已安装程序身份不符");
    let UpdateCheck::UpdateAvailable(info) = manager.check_for_updates()? else {
        anyhow::bail!("安装包没有可应用的新版本");
    };
    ensure!(
        info.TargetFullRelease
            .SHA256
            .eq_ignore_ascii_case(&release.asset.SHA256)
            && info.TargetFullRelease.FileName == release.asset.FileName,
        "待安装目标与签名声明不一致"
    );
    manager.download_updates(&info, None)?;
    // Velopack skips an existing package; independently revalidate that cache
    // before allowing any executable installation.
    let locator = velopack::locator::auto_locate_app_manifest(
        velopack::locator::LocationContext::FromCurrentExe,
    )?;
    verify_package(
        &locator.get_packages_dir().join(&release.asset.FileName),
        &release,
    )?;
    let backup_dir = store.root.join("upgrade-backups");
    std::fs::create_dir_all(&backup_dir)?;
    store.backup(&backup_dir.join(format!("{}.sqlite3", uuid::Uuid::new_v4())))?;
    let mut profile = store.profile()?;
    let revision = profile.revision;
    profile.auto_apply = false;
    store.update_profile(profile, revision)?;
    // Velopack waits for this process; no game resource operations are queued
    // after this call. Startup auto-install is disabled below.
    manager.apply_updates_and_restart(&*info)?;
    Ok(())
}
pub fn startup() {
    velopack::VelopackApp::build()
        .set_auto_apply_on_startup(false)
        .run();
}
