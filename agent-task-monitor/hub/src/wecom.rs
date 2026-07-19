//! 企业微信自建应用机器人：接收成员消息 → 指令调度 → 被动回复。
//! 走官方合规通道（自建应用回调），不碰个人微信 hook。
//!
//! 配置（全部来自环境变量，任一缺失则机器人路由静默停用）：
//!   AM_WECOM_TOKEN     自建应用「接收消息」的 Token
//!   AM_WECOM_AESKEY    EncodingAESKey（43 位）
//!   AM_WECOM_CORPID    企业 CorpID（回调里的 ToUserName/receiveid）
//!
//! 交互采用「被动回复」：收到消息后在 HTTP 响应里直接回一条加密文本，
//! 无需 access_token，也就无需 Secret —— 自用机器人最省心。

use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use sha1::{Digest, Sha1};

type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;
type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;

/// 机器人运行所需配置（从环境变量读取）
#[derive(Clone)]
pub struct WecomConfig {
    pub token: String,
    pub aes_key: Vec<u8>, // 32 字节
    pub corp_id: String,
}

impl WecomConfig {
    /// 企业微信配置（AM_WECOM_*）齐全才启用；缺任一项返回 None
    pub fn from_env() -> Option<Self> {
        Self::from_prefixed("AM_WECOM_TOKEN", "AM_WECOM_AESKEY", "AM_WECOM_CORPID")
    }

    /// 公众号配置（AM_MP_*）：receiveid = 公众号 AppID，其余同企业微信
    pub fn mp_from_env() -> Option<Self> {
        Self::from_prefixed("AM_MP_TOKEN", "AM_MP_AESKEY", "AM_MP_APPID")
    }

    fn from_prefixed(token_var: &str, aeskey_var: &str, id_var: &str) -> Option<Self> {
        let token = std::env::var(token_var).ok().filter(|s| !s.is_empty())?;
        let aeskey = std::env::var(aeskey_var).ok().filter(|s| !s.is_empty())?;
        let corp_id = std::env::var(id_var).ok().filter(|s| !s.is_empty())?;
        let aes_key = decode_aes_key(&aeskey)?;
        Some(Self { token, aes_key, corp_id })
    }
}

/// EncodingAESKey（43 字符）补 '=' 后 base64 解出 32 字节密钥
fn decode_aes_key(encoding_aes_key: &str) -> Option<Vec<u8>> {
    let key = B64.decode(format!("{encoding_aes_key}=")).ok()?;
    (key.len() == 32).then_some(key)
}

/// 企业微信消息签名：sha1( 排序拼接[token, timestamp, nonce, encrypt] )
pub fn msg_signature(token: &str, timestamp: &str, nonce: &str, encrypt: &str) -> String {
    let mut arr = [token, timestamp, nonce, encrypt];
    arr.sort_unstable();
    let mut hasher = Sha1::new();
    hasher.update(arr.concat().as_bytes());
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 公众号 URL 验证签名：sha1( 排序拼接[token, timestamp, nonce] )，不含 echostr
pub fn plain_signature(token: &str, timestamp: &str, nonce: &str) -> String {
    let mut arr = [token, timestamp, nonce];
    arr.sort_unstable();
    let mut hasher = Sha1::new();
    hasher.update(arr.concat().as_bytes());
    hex(&hasher.finalize())
}

/// 解密回调密文，返回 (明文, receiveid)。校验 receiveid 与 corp_id 一致。
pub fn decrypt(cfg: &WecomConfig, encrypt_b64: &str) -> Result<String, String> {
    let data = B64.decode(encrypt_b64.trim()).map_err(|_| "密文 base64 解码失败")?;
    if data.len() < 32 || data.len() % 16 != 0 {
        return Err("密文长度非法".into());
    }
    let iv = &cfg.aes_key[..16];
    let mut buf = data.clone();
    // 企业微信用 32 字节块的 PKCS7（补位值可达 32），标准 16 块 Pkcs7 解包会
    // 拒绝 >16 的补位 —— 必须 NoPadding 解密后手工去补位。
    let decrypted = Aes256CbcDec::new(cfg.aes_key.as_slice().into(), iv.into())
        .decrypt_padded_mut::<cbc::cipher::block_padding::NoPadding>(&mut buf)
        .map_err(|_| "AES 解密失败")?;
    let plain = wechat_unpad(decrypted)?;
    // 结构：[16 随机字节][4 字节 BE 消息长度][消息][receiveid]
    if plain.len() < 20 {
        return Err("明文过短".into());
    }
    let msg_len = u32::from_be_bytes([plain[16], plain[17], plain[18], plain[19]]) as usize;
    if 20 + msg_len > plain.len() {
        return Err("消息长度越界".into());
    }
    let msg = &plain[20..20 + msg_len];
    let receiveid = &plain[20 + msg_len..];
    if receiveid != cfg.corp_id.as_bytes() {
        return Err("receiveid 与 CorpID 不符".into());
    }
    String::from_utf8(msg.to_vec()).map_err(|_| "消息非 UTF-8".into())
}

/// 加密明文为回调密文（被动回复用）。random16 由调用方给（便于测试确定性）。
pub fn encrypt(cfg: &WecomConfig, msg: &str, random16: &[u8; 16]) -> String {
    // 明文：[16 随机][4 字节 BE 长度][消息][receiveid]，再按 32 块 PKCS7 手工补位
    let mut plain = Vec::with_capacity(16 + 4 + msg.len() + cfg.corp_id.len() + 32);
    plain.extend_from_slice(random16);
    plain.extend_from_slice(&(msg.len() as u32).to_be_bytes());
    plain.extend_from_slice(msg.as_bytes());
    plain.extend_from_slice(cfg.corp_id.as_bytes());
    wechat_pad(&mut plain);
    let iv = &cfg.aes_key[..16];
    let len = plain.len();
    let ct = Aes256CbcEnc::new(cfg.aes_key.as_slice().into(), iv.into())
        .encrypt_padded_mut::<cbc::cipher::block_padding::NoPadding>(&mut plain, len)
        .expect("cbc 加密不应失败");
    B64.encode(ct)
}

/// 企业微信 PKCS7：块大小 32，补 N 个值为 N 的字节（N ∈ 1..=32）
fn wechat_pad(buf: &mut Vec<u8>) {
    let pad = 32 - (buf.len() % 32);
    let pad = if pad == 0 { 32 } else { pad };
    buf.extend(std::iter::repeat(pad as u8).take(pad));
}

fn wechat_unpad(data: &[u8]) -> Result<&[u8], String> {
    let n = *data.last().ok_or("空明文")? as usize;
    if n == 0 || n > 32 || n > data.len() {
        return Err("补位长度非法".into());
    }
    Ok(&data[..data.len() - n])
}

/// 从 XML 里取某个标签的文本（够用的极简解析：企业微信回调是扁平 XML，
/// 且值多包在 CDATA 里）。不引 XML 库，避免为一个固定格式增加依赖。
pub fn xml_field(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    let raw = &xml[start..end];
    let raw = raw
        .strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .unwrap_or(raw);
    Some(raw.to_string())
}

/// 组装被动回复的外层加密 XML
pub fn build_reply(cfg: &WecomConfig, msg: &str, timestamp: &str, nonce: &str, random16: &[u8; 16]) -> String {
    let encrypt = encrypt(cfg, msg, random16);
    let sig = msg_signature(&cfg.token, timestamp, nonce, &encrypt);
    format!(
        "<xml><Encrypt><![CDATA[{encrypt}]]></Encrypt>\
         <MsgSignature><![CDATA[{sig}]]></MsgSignature>\
         <TimeStamp>{timestamp}</TimeStamp>\
         <Nonce><![CDATA[{nonce}]]></Nonce></xml>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> WecomConfig {
        // 用真实 32 字节密钥编码出合法的 43 位 EncodingAESKey，再走生产解码路径，
        // 顺带验证 decode_aes_key 本身
        let raw = [7u8; 32];
        let encoding = B64.encode(raw);
        let encoding = encoding.trim_end_matches('='); // 43 位
        assert_eq!(encoding.len(), 43);
        WecomConfig {
            token: "QDG6eK".into(),
            aes_key: decode_aes_key(encoding).expect("43 位 key 应解出 32 字节"),
            corp_id: "wx5823bde119abcdef".into(),
        }
    }

    #[test]
    fn signature_is_sorted_sha1() {
        // 手工核对：排序后拼接再 sha1
        let s = msg_signature("QDG6eK", "1409659813", "1372623149", "encblob");
        let mut a = ["QDG6eK", "1409659813", "1372623149", "encblob"];
        a.sort_unstable();
        let mut h = Sha1::new();
        h.update(a.concat().as_bytes());
        assert_eq!(s, hex(&h.finalize()));
        assert_eq!(s.len(), 40);
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let c = cfg();
        let msg = "会话";
        let enc = encrypt(&c, msg, &[7u8; 16]);
        assert_eq!(decrypt(&c, &enc).unwrap(), msg);
        // receiveid 不符必须拒绝
        let mut wrong = c.clone();
        wrong.corp_id = "othercorp".into();
        assert!(decrypt(&wrong, &enc).is_err());
    }

    #[test]
    fn roundtrip_multibyte_and_long() {
        let c = cfg();
        let msg = "暂停 3\n已发送：给用户表加索引 ✅ 长文本".repeat(20);
        let enc = encrypt(&c, &msg, &[0u8; 16]);
        assert_eq!(decrypt(&c, &enc).unwrap(), msg);
    }

    #[test]
    fn xml_field_cdata() {
        let xml = "<xml><FromUserName><![CDATA[zhangsan]]></FromUserName><Content><![CDATA[绑定 A1B2]]></Content><MsgType><![CDATA[text]]></MsgType></xml>";
        assert_eq!(xml_field(xml, "FromUserName").unwrap(), "zhangsan");
        assert_eq!(xml_field(xml, "Content").unwrap(), "绑定 A1B2");
        assert_eq!(xml_field(xml, "MsgType").unwrap(), "text");
        assert!(xml_field(xml, "Absent").is_none());
    }
}
