//! 令牌与短 id。
//!
//! 不引 uuid / base64：`getrandom` 出字节、手写 hex 编码就够了，而且 hex 是
//! URL 安全字符集里最没有歧义的一种（大小写不敏感、不含 `-` `_`）。
//!
//! 长度选择：
//! - **链接令牌 16 字节 = 128 位**。要放进文章里，32 个字符已经是可接受的上限；
//!   再长（比如 32 字节 = 64 字符）安全边际没有实际意义 —— 链接是公开的，
//!   防的是「猜中别人的令牌」，128 位已经远超暴力枚举的能力。
//! - **短 id 4 字节**。它只用于 API 里指认来源/消息，不是凭证；32 位空间在
//!   50 条链接的规模下碰撞概率可以忽略。

use std::fmt::Write;

/// `bytes` 个随机字节的 hex 表示（长度 = `2 * bytes`）。
fn hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("系统随机源不可用");
    let mut out = String::with_capacity(bytes * 2);
    for byte in buf {
        write!(out, "{byte:02x}").expect("写 String 不会失败");
    }
    out
}

/// 公开链接里的令牌（16 字节 = 128 位）。
pub fn token() -> String {
    hex(16)
}

/// 来源 / 消息的短 id（4 字节）。只用于指认，不承担安全职责。
pub fn short_id() -> String {
    hex(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_32_hex_chars() {
        let t = token();
        assert_eq!(t.len(), 32);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn ids_are_unique_enough() {
        let seen: std::collections::HashSet<String> = (0..1000).map(|_| token()).collect();
        assert_eq!(seen.len(), 1000, "1000 次生成不该有重复");
    }

    #[test]
    fn short_id_is_8_hex_chars() {
        let id = short_id();
        assert_eq!(id.len(), 8);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
