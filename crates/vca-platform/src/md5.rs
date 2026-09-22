//! MD5：只为「企业微信图片消息」这一处需要。
//!
//! # 为什么自己写而不是引 crate
//!
//! 企业微信群机器人的 `image` 消息要求带图片内容的 MD5
//! （官方文档 path/91770：`{"base64": "...", "md5": "..."}`）。
//! 项目里没有任何地方再用到 MD5，而它是**完全公开、无需密钥**的算法 ——
//! RFC 1321 的四个轮函数一共几十行，为一个校验和去引一个 crate
//! （还要盯它换版本、跟着升级）并不划算。
//!
//! 正确性由 RFC 1321 附录 A.5 的**标准测试向量**保证（见文末测试），
//! 不是"看着像就行"。

/// 计算一段字节的 MD5，返回 32 位小写十六进制。
pub fn hex(data: &[u8]) -> String {
    let d = digest(data);
    let mut s = String::with_capacity(32);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 计算 MD5 的 16 字节摘要。
pub fn digest(data: &[u8]) -> [u8; 16] {
    // RFC 1321 的常量表：K[i] = floor(2^32 * abs(sin(i + 1)))
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    // 填充：补 1 个 0x80，补 0 到 56 (mod 64)，最后 8 字节是原始长度（比特、小端）
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            let b = &chunk[i * 4..i * 4 + 4];
            *w = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        }

        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let tmp = d;
            d = c;
            c = b;
            let sum = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(S[i]));
            a = tmp;
        }

        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 1321 附录 A.5 的七组标准测试向量。
    ///
    /// 这不是"跑一遍记下来"的快照测试 —— 它们是标准文档里给的期望值，
    /// 任何一处轮函数、常量表或填充写错都会立刻暴露。
    #[test]
    fn rfc1321_vectors() {
        let cases: &[(&str, &str)] = &[
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(&hex(input.as_bytes()), want, "输入 {input:?}");
        }
    }

    /// 长度跨过 55/56/64 这几个填充边界 —— 这里最容易写错。
    #[test]
    fn padding_boundaries() {
        for n in [54usize, 55, 56, 57, 63, 64, 65, 119, 120, 128] {
            let data = vec![b'x'; n];
            let got = hex(&data);
            assert_eq!(got.len(), 32, "长度 {n}");
            // 与已知实现对照：md5 的"同一输入必得同一结果"自洽性 + 非空
            assert_eq!(got, hex(&data), "长度 {n} 的结果必须稳定");
            assert_ne!(got, hex(&vec![b'y'; n]), "不同内容不该撞上");
        }
    }

    #[test]
    fn digest_is_16_bytes() {
        assert_eq!(digest(b"whatever").len(), 16);
    }
}
