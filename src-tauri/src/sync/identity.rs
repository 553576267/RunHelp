//! 设备身份：自签证书 + 指纹固定（certificate pinning）。
//!
//! 每台设备首次启动生成一张自签证书，持久化到 app_data_dir/sync_identity/。
//! 证书的 SHA-256 指纹就是设备身份证：私钥只在本机，无法伪造；设备名随便改都不影响身份。
//!
//! TLS 握手：双方交换证书，verifier 只验证「对方确实持有该证书私钥」（签名校验），
//! 不做 CA 链验证（自签证书没有 CA）。身份互验放在应用层：
//!   - 每次请求带 X-RH-Fingerprint = 本机证书指纹；
//!   - 接收方比对「声明指纹 == 握手证书指纹」，一致才认人，防伪造；
//!   - 首次传输弹窗让用户确认（TOFU），确认后写入 trusted 集合，之后免确认。

use std::path::Path;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, Error as TlsError, SignatureScheme};

use sha2::{Digest, Sha256};

const IDENTITY_DIR: &str = "sync_identity";
const CERT_FILE: &str = "device_cert.der";
const KEY_FILE: &str = "device_key.pem";

/// 本机身份：证书 DER + 私钥 PEM + 指纹
#[derive(Clone)]
pub struct Identity {
    pub cert_der: CertificateDer<'static>,
    key_pem: String,
    /// 完整指纹（64 位 hex）
    pub fingerprint: String,
}

impl Identity {
    /// 分组展示指纹：4-4-4-4（取前 12 位），方便阅读比对
    pub fn grouped(&self) -> String {
        let s: Vec<char> = self.fingerprint.chars().take(12).collect();
        format!(
            "{}-{}-{}",
            s[0..4].iter().collect::<String>(),
            s[4..8].iter().collect::<String>(),
            s[8..12].iter().collect::<String>(),
        )
    }
}

/// 加载或生成设备身份。证书/私钥落盘，重启后指纹不变。
pub fn load_or_create(data_dir: &Path) -> Result<Identity, String> {
    let dir = data_dir.join(IDENTITY_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let cert_path = dir.join(CERT_FILE);
    let key_path = dir.join(KEY_FILE);

    let (cert_der, key_pem) = if cert_path.is_file() && key_path.is_file() {
        let der = std::fs::read(&cert_path).map_err(|e| e.to_string())?;
        let key = std::fs::read_to_string(&key_path).map_err(|e| e.to_string())?;
        (CertificateDer::from(der), key)
    } else {
        create_and_save(&cert_path, &key_path)?
    };

    let fingerprint = fingerprint_of(&cert_der);
    Ok(Identity { cert_der, key_pem, fingerprint })
}

fn create_and_save(cert_path: &Path, key_path: &Path) -> Result<(CertificateDer<'static>, String), String> {
    let mut params = rcgen::CertificateParams::new(vec![
        "runhelp.local".to_string(),
        "localhost".to_string(),
    ])
    .map_err(|e| e.to_string())?;
    params.distinguished_name.push(rcgen::DnType::CommonName, "RunHelp Device");
    let key_pair = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = params.self_signed(&key_pair).map_err(|e| e.to_string())?;

    std::fs::write(cert_path, cert.der().as_ref()).map_err(|e| e.to_string())?;
    let key_pem = key_pair.serialize_pem();
    std::fs::write(key_path, key_pem.as_bytes()).map_err(|e| e.to_string())?;
    Ok((cert.der().clone(), key_pem))
}

/// 计算证书 DER 的 SHA-256 指纹（hex）
pub fn fingerprint_of(cert: &CertificateDer<'_>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(cert.as_ref());
    hex::encode(hasher.finalize())
}

/// 删除本机身份（重新生成指纹时用）
pub fn reset(data_dir: &Path) -> Result<(), String> {
    let dir = data_dir.join(IDENTITY_DIR);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ===== TLS config：宽松握手（验签名，不验 CA 链），身份在应用层靠指纹判定 =====

pub fn server_config(identity: &Identity) -> Result<rustls::ServerConfig, String> {
    let key = key_from_pem(&identity.key_pem)?;
    rustls::ServerConfig::builder()
        .with_client_cert_verifier(Arc::new(SignatureOnlyClientVerifier::new()))
        .with_single_cert(vec![identity.cert_der.clone()], key)
        .map_err(|e| format!("构造 TLS 服务端配置失败: {}", e))
}

pub fn client_config(identity: &Identity) -> Result<rustls::ClientConfig, String> {
    let key = key_from_pem(&identity.key_pem)?;
    rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SignatureOnlyServerVerifier::new()))
        .with_client_auth_cert(vec![identity.cert_der.clone()], key)
        .map_err(|e| format!("构造 TLS 客户端配置失败: {}", e))
}

use std::sync::Arc;

fn key_from_pem(pem: &str) -> Result<PrivateKeyDer<'static>, String> {
    let kp = rcgen::KeyPair::from_pem(pem).map_err(|e| format!("解析私钥失败: {}", e))?;
    Ok(PrivateKeyDer::Pkcs8(kp.serialize_der().into()))
}

fn sig_schemes() -> rustls::crypto::WebPkiSupportedAlgorithms {
    rustls::crypto::ring::default_provider().signature_verification_algorithms
}

/// 只验证「客户端持有证书私钥」，不校验指纹/CA。指纹在应用层判定。
#[derive(Debug)]
struct SignatureOnlyClientVerifier {
    schemes: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl SignatureOnlyClientVerifier {
    fn new() -> Self {
        Self { schemes: sig_schemes() }
    }
}

impl ClientCertVerifier for SignatureOnlyClientVerifier {
    fn offer_client_auth(&self) -> bool { true }
    fn client_auth_mandatory(&self) -> bool { true }
    fn root_hint_subjects(&self) -> &[DistinguishedName] { &[] }
    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, TlsError> {
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.schemes)
    }
    fn verify_tls13_signature(
        &self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.schemes)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes.supported_schemes()
    }
}

/// 只验证「服务端持有证书私钥」，不校验指纹/CA。
#[derive(Debug)]
struct SignatureOnlyServerVerifier {
    schemes: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl SignatureOnlyServerVerifier {
    fn new() -> Self {
        Self { schemes: sig_schemes() }
    }
}

impl ServerCertVerifier for SignatureOnlyServerVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.schemes)
    }
    fn verify_tls13_signature(
        &self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.schemes)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes.supported_schemes()
    }
}
