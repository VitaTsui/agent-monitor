use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes128Gcm, Aes256Gcm, Nonce};
use anyhow::{anyhow, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rsa::{Oaep, RsaPrivateKey};
use sha1::Sha1;
use sha2::Sha256;

/// AES/GCM/NoPadding，12 字节 IV 前置，整体 base64 —— 与前端 crypto.ts 对齐
pub fn aes_gcm_encrypt(plain: &str, key: &str) -> Result<String> {
    let kb = key.as_bytes();
    let (nonce, mut out) = match kb.len() {
        16 => {
            let cipher = Aes128Gcm::new_from_slice(kb).map_err(|e| anyhow!("AES key: {e}"))?;
            let nonce = Aes128Gcm::generate_nonce(&mut OsRng);
            let ct = cipher
                .encrypt(&nonce, plain.as_bytes())
                .map_err(|e| anyhow!("AES 加密失败: {e}"))?;
            (nonce.to_vec(), ct)
        }
        32 => {
            let cipher = Aes256Gcm::new_from_slice(kb).map_err(|e| anyhow!("AES key: {e}"))?;
            let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
            let ct = cipher
                .encrypt(&nonce, plain.as_bytes())
                .map_err(|e| anyhow!("AES 加密失败: {e}"))?;
            (nonce.to_vec(), ct)
        }
        n => return Err(anyhow!("AES 密钥长度须为 16/32 字节，当前 {n}")),
    };
    let mut buf = nonce;
    buf.append(&mut out);
    Ok(B64.encode(buf))
}

pub fn aes_gcm_decrypt(b64: &str, key: &str) -> Result<String> {
    let raw = B64.decode(b64.trim())?;
    if raw.len() < 13 {
        return Err(anyhow!("密文过短"));
    }
    let (iv, ct) = raw.split_at(12);
    let nonce = Nonce::from_slice(iv);
    let kb = key.as_bytes();
    let plain = match kb.len() {
        16 => Aes128Gcm::new_from_slice(kb)
            .map_err(|e| anyhow!("AES key: {e}"))?
            .decrypt(nonce, ct)
            .map_err(|_| anyhow!("AES 解密失败"))?,
        32 => Aes256Gcm::new_from_slice(kb)
            .map_err(|e| anyhow!("AES key: {e}"))?
            .decrypt(nonce, ct)
            .map_err(|_| anyhow!("AES 解密失败"))?,
        n => return Err(anyhow!("AES 密钥长度须为 16/32 字节，当前 {n}")),
    };
    Ok(String::from_utf8(plain)?)
}

/// RSA OAEP：摘要 SHA-256、MGF1 用 SHA-1（对齐 OpenJDK 默认行为与前端注释）
pub fn rsa_decrypt(b64: &str, private_key: &RsaPrivateKey) -> Result<String> {
    let raw = B64.decode(b64.trim())?;
    let padding = Oaep::new_with_mgf_hash::<Sha256, Sha1>();
    let plain = private_key
        .decrypt(padding, &raw)
        .map_err(|_| anyhow!("RSA 解密失败"))?;
    Ok(String::from_utf8(plain)?)
}

/// 登录字段解密：AES-decrypt( RSA-decrypt(field), sessionKey )
pub fn decrypt_login_field(
    field: &str,
    session_key: &str,
    private_key: &RsaPrivateKey,
) -> Result<String> {
    let aes_blob = rsa_decrypt(field, private_key)?;
    aes_gcm_decrypt(&aes_blob, session_key)
}
