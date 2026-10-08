//! Signed data-only hot updates. Public release trust is configured by the
//! distributor, never learned from the downloaded envelope itself.
use crate::Store;
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, VerifyingKey};
use poe2_core::AnnotationRules;
use serde::{Deserialize, Serialize};

const LIMIT: usize = 64 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSource {
    pub feed_url: String,
    pub public_key_hex: String,
    pub channel: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleRelease {
    pub protocol: u32,
    pub channel: String,
    pub sequence: u64,
    pub version: String,
    pub engine_protocol: u32,
    pub published_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub rules: AnnotationRules,
}
/// Signature covers the exact UTF-8 bytes of payload, prefixed with the
/// domain separator. No JSON reserialization/canonicalization ambiguity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedRules {
    pub payload: String,
    pub signature_hex: String,
}
pub fn signing_bytes(payload: &str) -> Vec<u8> {
    let mut bytes = b"poe2-toolkit/rules/v1\0".to_vec();
    bytes.extend_from_slice(payload.as_bytes());
    bytes
}
impl UpdateSource {
    pub fn validate(&self) -> Result<()> {
        let url = reqwest::Url::parse(&self.feed_url)?;
        ensure!(
            url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none(),
            "发行源必须是无凭据的 HTTPS 地址"
        );
        ensure!(
            !self.channel.is_empty()
                && self.channel.len() <= 32
                && self
                    .channel
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
            "发行通道无效"
        );
        self.key()?;
        Ok(())
    }
    pub(crate) fn key(&self) -> Result<VerifyingKey> {
        let bytes: [u8; 32] = hex::decode(&self.public_key_hex)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("公钥必须为 32 字节"))?;
        let key = VerifyingKey::from_bytes(&bytes)?;
        ensure!(!key.is_weak(), "发行公钥无效");
        Ok(key)
    }
}
pub fn verify(
    source: &UpdateSource,
    envelope: &SignedRules,
    now: DateTime<Utc>,
) -> Result<RuleRelease> {
    source.validate()?;
    ensure!(
        envelope.payload.len() <= LIMIT && envelope.signature_hex.len() == 128,
        "规则包大小或签名长度无效"
    );
    let signature = Signature::from_slice(&hex::decode(&envelope.signature_hex)?)?;
    source
        .key()?
        .verify_strict(&signing_bytes(&envelope.payload), &signature)
        .context("规则包签名校验失败")?;
    let release: RuleRelease = serde_json::from_str(&envelope.payload)?;
    ensure!(
        release.protocol == 1 && release.engine_protocol == 2,
        "规则协议与当前应用不兼容"
    );
    ensure!(release.channel == source.channel, "规则包发行通道不匹配");
    ensure!(
        release.sequence > 0 && !release.version.is_empty() && release.version.len() <= 64,
        "规则版本无效"
    );
    ensure!(
        release.published_at <= now
            && release.expires_at > now
            && release.expires_at > release.published_at,
        "规则包尚未生效或已过期"
    );
    ensure!(release.rules.valid(), "规则参数越出允许范围");
    Ok(release)
}
pub fn active_rules(store: &Store) -> Result<(String, AnnotationRules)> {
    if let (Some(source), Some(envelope)) = (
        store.get::<UpdateSource>("update_source")?,
        store.get::<SignedRules>("active_rules")?,
    ) {
        // Expired rules cannot silently remain active. Builtin rules are always
        // available; the caller compares the resulting version with its plan.
        if let Ok(release) = verify(&source, &envelope, Utc::now()) {
            return Ok((
                format!(
                    "signed:{}:{}",
                    release.sequence,
                    poe2_core::digest(envelope.payload.as_bytes())
                ),
                release.rules,
            ));
        }
    }
    Ok(("builtin-rules-v2".into(), AnnotationRules::default()))
}
pub fn configure(store: &Store, source: UpdateSource) -> Result<()> {
    source.validate()?;
    // Keep sequence history across source configuration so changing an URL
    // cannot re-enable a previously rejected version on the same channel.
    store.set("update_source", &source)
}
pub fn activate(store: &Store, envelope: &SignedRules) -> Result<RuleRelease> {
    let source: UpdateSource = store
        .get("update_source")?
        .context("尚未配置受信任发行源")?;
    let release = verify(&source, envelope, Utc::now())?;
    store.activate_rules(envelope, &release)?;
    Ok(release)
}
pub async fn refresh(store: &Store) -> Result<RuleRelease> {
    let source: UpdateSource = store
        .get("update_source")?
        .context("尚未配置受信任发行源")?;
    source.validate()?;
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let mut response = client
        .get(&source.feed_url)
        .send()
        .await?
        .error_for_status()?;
    ensure!(
        response.content_length().is_none_or(|n| n <= LIMIT as u64),
        "规则包超过大小上限"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(bytes.len() + chunk.len() <= LIMIT, "规则包超过大小上限");
        bytes.extend_from_slice(&chunk);
    }
    let envelope: SignedRules = serde_json::from_slice(&bytes)?;
    if let Some(active) = store.get::<SignedRules>("active_rules")?
        && active.payload == envelope.payload
        && active.signature_hex == envelope.signature_hex
    {
        let current: UpdateSource = store.get("update_source")?.context("发行源配置发生变化")?;
        return verify(&current, &envelope, Utc::now());
    }
    activate(store, &envelope)
}
