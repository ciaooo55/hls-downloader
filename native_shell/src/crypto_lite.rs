//! AES-128 CBC (PKCS7) and SHA-256 without extra crates.

pub fn decrypt_aes128_cbc_pkcs7(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    if key.len() != 16 || iv.len() != 16 {
        return Err("AES-128 key/IV must be 16 bytes".into());
    }
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return Err("AES-128 ciphertext must be a multiple of 16 bytes".into());
    }
    let round_keys = expand_key(key);
    let mut prev = [0u8; 16];
    prev.copy_from_slice(iv);
    let mut plain = Vec::with_capacity(data.len());
    for chunk in data.as_chunks::<16>().0 {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        decrypt_block(&round_keys, &mut block);
        for (byte, prev_byte) in block.iter_mut().zip(prev.iter()) {
            *byte ^= prev_byte;
        }
        prev.copy_from_slice(chunk);
        plain.extend_from_slice(&block);
    }
    let pad = *plain.last().unwrap_or(&0) as usize;
    if pad == 0 || pad > 16 || plain.len() < pad {
        return Err("invalid PKCS7 padding".into());
    }
    if plain[plain.len() - pad..]
        .iter()
        .any(|byte| *byte as usize != pad)
    {
        return Err("invalid PKCS7 padding".into());
    }
    plain.truncate(plain.len() - pad);
    Ok(plain)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    sha256(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub struct Sha256Hasher {
    state: [u32; 8],
    buffer: [u8; 64],
    filled: usize,
    total_bytes: u64,
}

impl Default for Sha256Hasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256Hasher {
    pub fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: [0; 64],
            filled: 0,
            total_bytes: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total_bytes += data.len() as u64;
        if self.filled > 0 {
            let take = (64 - self.filled).min(data.len());
            self.buffer[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == 64 {
                sha256_compress(&mut self.state, &self.buffer);
                self.filled = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            sha256_compress(&mut self.state, &block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.filled = data.len();
        }
    }

    pub fn finish(mut self) -> [u8; 32] {
        let bit_len = self.total_bytes * 8;
        self.buffer[self.filled] = 0x80;
        self.filled += 1;
        if self.filled > 56 {
            for slot in self.buffer.iter_mut().skip(self.filled) {
                *slot = 0;
            }
            sha256_compress(&mut self.state, &self.buffer);
            self.filled = 0;
        }
        for slot in self.buffer.iter_mut().take(56).skip(self.filled) {
            *slot = 0;
        }
        self.buffer[56..64].copy_from_slice(&bit_len.to_be_bytes());
        sha256_compress(&mut self.state, &self.buffer);
        let mut out = [0u8; 32];
        for (index, word) in self.state.iter().enumerate() {
            out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h = [
        0x67452301u32,
        0xEFCDAB89,
        0x98BADCFE,
        0x10325476,
        0xC3D2E1F0,
    ];
    let bit_len = (data.len() as u64) * 8;
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (index, part) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[index] = u32::from_be_bytes(*part);
        }
        for index in 16..80 {
            w[index] = (w[index - 3] ^ w[index - 8] ^ w[index - 14] ^ w[index - 16]).rotate_left(1);
        }
        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        for (index, word) in w.iter().enumerate() {
            let (f, k) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (index, word) in h.iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

pub fn sha1_hex(bytes: &[u8]) -> String {
    sha1(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn expand_key(key: &[u8]) -> [u32; 44] {
    let mut w = [0u32; 44];
    for i in 0..4 {
        w[i] = u32::from_be_bytes(key[i * 4..i * 4 + 4].try_into().unwrap());
    }
    for i in 4..44 {
        let mut temp = w[i - 1];
        if i % 4 == 0 {
            temp = sub_word(temp.rotate_left(8)) ^ RCON[i / 4 - 1];
        }
        w[i] = w[i - 4] ^ temp;
    }
    w
}

fn decrypt_block(round_keys: &[u32; 44], block: &mut [u8; 16]) {
    add_round_key(block, &round_keys[40..44]);
    inv_shift_rows(block);
    inv_sub_bytes(block);
    for round in (1..10).rev() {
        add_round_key(block, &round_keys[round * 4..round * 4 + 4]);
        inv_mix_columns(block);
        inv_shift_rows(block);
        inv_sub_bytes(block);
    }
    add_round_key(block, &round_keys[0..4]);
}

fn add_round_key(block: &mut [u8; 16], words: &[u32]) {
    for (index, word) in words.iter().enumerate() {
        let bytes = word.to_be_bytes();
        for (offset, byte) in bytes.iter().enumerate() {
            block[index * 4 + offset] ^= byte;
        }
    }
}

fn inv_sub_bytes(block: &mut [u8; 16]) {
    for byte in block.iter_mut() {
        *byte = INV_SBOX[*byte as usize];
    }
}

fn inv_shift_rows(block: &mut [u8; 16]) {
    let copy = *block;
    for row in 1..4 {
        for col in 0..4 {
            block[row + 4 * col] = copy[row + 4 * ((col + 4 - row) % 4)];
        }
    }
}

fn inv_mix_columns(block: &mut [u8; 16]) {
    for col in 0..4 {
        let i = col * 4;
        let a0 = block[i];
        let a1 = block[i + 1];
        let a2 = block[i + 2];
        let a3 = block[i + 3];
        block[i] = gmul(a0, 14) ^ gmul(a1, 11) ^ gmul(a2, 13) ^ gmul(a3, 9);
        block[i + 1] = gmul(a0, 9) ^ gmul(a1, 14) ^ gmul(a2, 11) ^ gmul(a3, 13);
        block[i + 2] = gmul(a0, 13) ^ gmul(a1, 9) ^ gmul(a2, 14) ^ gmul(a3, 11);
        block[i + 3] = gmul(a0, 11) ^ gmul(a1, 13) ^ gmul(a2, 9) ^ gmul(a3, 14);
    }
}

fn sub_word(word: u32) -> u32 {
    let bytes = word.to_be_bytes();
    u32::from_be_bytes([
        SBOX[bytes[0] as usize],
        SBOX[bytes[1] as usize],
        SBOX[bytes[2] as usize],
        SBOX[bytes[3] as usize],
    ])
}

fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 {
            p ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    p
}

fn sha256(message: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256Hasher::new();
    hasher.update(message);
    hasher.finish()
}

fn sha256_compress(state: &mut [u32; 8], chunk: &[u8; 64]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let mut a = *state;
    for i in 0..64 {
        let s1 = a[4].rotate_right(6) ^ a[4].rotate_right(11) ^ a[4].rotate_right(25);
        let ch = (a[4] & a[5]) ^ ((!a[4]) & a[6]);
        let temp1 = a[7]
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a[0].rotate_right(2) ^ a[0].rotate_right(13) ^ a[0].rotate_right(22);
        let maj = (a[0] & a[1]) ^ (a[0] & a[2]) ^ (a[1] & a[2]);
        let temp2 = s0.wrapping_add(maj);
        a[7] = a[6];
        a[6] = a[5];
        a[5] = a[4];
        a[4] = a[3].wrapping_add(temp1);
        a[3] = a[2];
        a[2] = a[1];
        a[1] = a[0];
        a[0] = temp1.wrapping_add(temp2);
    }
    for (slot, value) in state.iter_mut().zip(a.iter()) {
        *slot = slot.wrapping_add(*value);
    }
}

const RCON: [u32; 10] = [
    0x01000000, 0x02000000, 0x04000000, 0x08000000, 0x10000000, 0x20000000, 0x40000000, 0x80000000,
    0x1b000000, 0x36000000,
];

const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

const INV_SBOX: [u8; 256] = [
    0x52, 0x09, 0x6a, 0xd5, 0x30, 0x36, 0xa5, 0x38, 0xbf, 0x40, 0xa3, 0x9e, 0x81, 0xf3, 0xd7, 0xfb,
    0x7c, 0xe3, 0x39, 0x82, 0x9b, 0x2f, 0xff, 0x87, 0x34, 0x8e, 0x43, 0x44, 0xc4, 0xde, 0xe9, 0xcb,
    0x54, 0x7b, 0x94, 0x32, 0xa6, 0xc2, 0x23, 0x3d, 0xee, 0x4c, 0x95, 0x0b, 0x42, 0xfa, 0xc3, 0x4e,
    0x08, 0x2e, 0xa1, 0x66, 0x28, 0xd9, 0x24, 0xb2, 0x76, 0x5b, 0xa2, 0x49, 0x6d, 0x8b, 0xd1, 0x25,
    0x72, 0xf8, 0xf6, 0x64, 0x86, 0x68, 0x98, 0x16, 0xd4, 0xa4, 0x5c, 0xcc, 0x5d, 0x65, 0xb6, 0x92,
    0x6c, 0x70, 0x48, 0x50, 0xfd, 0xed, 0xb9, 0xda, 0x5e, 0x15, 0x46, 0x57, 0xa7, 0x8d, 0x9d, 0x84,
    0x90, 0xd8, 0xab, 0x00, 0x8c, 0xbc, 0xd3, 0x0a, 0xf7, 0xe4, 0x58, 0x05, 0xb8, 0xb3, 0x45, 0x06,
    0xd0, 0x2c, 0x1e, 0x8f, 0xca, 0x3f, 0x0f, 0x02, 0xc1, 0xaf, 0xbd, 0x03, 0x01, 0x13, 0x8a, 0x6b,
    0x3a, 0x91, 0x11, 0x41, 0x4f, 0x67, 0xdc, 0xea, 0x97, 0xf2, 0xcf, 0xce, 0xf0, 0xb4, 0xe6, 0x73,
    0x96, 0xac, 0x74, 0x22, 0xe7, 0xad, 0x35, 0x85, 0xe2, 0xf9, 0x37, 0xe8, 0x1c, 0x75, 0xdf, 0x6e,
    0x47, 0xf1, 0x1a, 0x71, 0x1d, 0x29, 0xc5, 0x89, 0x6f, 0xb7, 0x62, 0x0e, 0xaa, 0x18, 0xbe, 0x1b,
    0xfc, 0x56, 0x3e, 0x4b, 0xc6, 0xd2, 0x79, 0x20, 0x9a, 0xdb, 0xc0, 0xfe, 0x78, 0xcd, 0x5a, 0xf4,
    0x1f, 0xdd, 0xa8, 0x33, 0x88, 0x07, 0xc7, 0x31, 0xb1, 0x12, 0x10, 0x59, 0x27, 0x80, 0xec, 0x5f,
    0x60, 0x51, 0x7f, 0xa9, 0x19, 0xb5, 0x4a, 0x0d, 0x2d, 0xe5, 0x7a, 0x9f, 0x93, 0xc9, 0x9c, 0xef,
    0xa0, 0xe0, 0x3b, 0x4d, 0xae, 0x2a, 0xf5, 0xb0, 0xc8, 0xeb, 0xbb, 0x3c, 0x83, 0x53, 0x99, 0x61,
    0x17, 0x2b, 0x04, 0x7e, 0xba, 0x77, 0xd6, 0x26, 0xe1, 0x69, 0x14, 0x63, 0x55, 0x21, 0x0c, 0x7d,
];

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_empty_matches_known_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha1_empty_matches_known_vector() {
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn sha256_hasher_matches_one_shot() {
        let mut hasher = Sha256Hasher::new();
        hasher.update(b"abc");
        hasher.update(b"def");
        assert_eq!(
            hasher
                .finish()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            sha256_hex(b"abcdef")
        );
    }

    // 手写 SHA 的 padding 与分块边界是最容易出错的地方，而 BT piece 校验（SHA-1）
    // 与任务校验和（SHA-256）都依赖它。下面用 Python hashlib 生成的已知答案向量，
    // 覆盖 55/56/57/63/64/65/119/120 这些块边界长度。
    fn pattern(length: usize) -> Vec<u8> {
        (0..length)
            .map(|index| ((index * 31 + 7) % 256) as u8)
            .collect()
    }

    const SHA256_ONE_SHOT: &[(usize, &str)] = &[
        (
            0,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            1,
            "ca358758f6d27e6cf45272937977a748fd88391db679ceda7dc7bf1f005ee879",
        ),
        (
            2,
            "140d811b81973993df99b8b1742b383ab83f6f52bf7af850812e7bba02ff11da",
        ),
        (
            3,
            "647674a296197442f518bcca323ec605dd8d098b2d4f22ee1fdcdd2bb753a189",
        ),
        (
            31,
            "5e5f9fa56d6337115e86a2508477e87c7d5296d0b0743ecfde2d0caeed2db37d",
        ),
        (
            32,
            "8e889f10b21cdd1b3ad72f740317a827d76e1b5b3f721e33c566f06d1deff8ea",
        ),
        (
            55,
            "8aa994584139d128848eeebc4e815639ba5ab6e6e39574195a63ac4f14f7c43b",
        ),
        (
            56,
            "ad574708f75c044c9b85de64cb568ee7711ff4f36448c6242f053ba8f6cc2b63",
        ),
        (
            57,
            "5b46e502092be01b1100193e089fdda95638c12e19a1d24f308eb2c3d3ae849d",
        ),
        (
            63,
            "280ed3e8ff1df845b2e7dfe6ac6cee817bef20e783cc65abc41b818b4d2fe076",
        ),
        (
            64,
            "c6ab9724ade5b6a7a1edfffb12f3aa9181351355af8fd08c919952ad211339dd",
        ),
        (
            65,
            "788367c73c7ddf4c53f65e68cc0d943e6227ab55b0e78ba63ace822b1c6301c0",
        ),
        (
            111,
            "dd1413178fb627f9abbc041ffe39c44aa7aaa0e2e6d2ca5c4528ac7073a2da45",
        ),
        (
            119,
            "3d610547d68216dedf7435a4fb6260353911f6b3fd3f18805ddb8be285d726fe",
        ),
        (
            120,
            "1f80156a804cb7862ad113e8200e9d74499723e7c7854d5f48776d3148e09656",
        ),
        (
            127,
            "192409cd280e14b743642ad1343fbd3e82d9305de72c078117745a679210cc3d",
        ),
        (
            128,
            "cc548ca2dec1f6fe4f58b2e27aa9c7521607df1130d140b55a4dad0665302356",
        ),
        (
            129,
            "81e89a7b2911aaa7795f9e3d4910cb47d6cd2b00d83b8399481527261a1a7519",
        ),
        (
            191,
            "2a30958d124d569d0a4832c608c772181557edbae684ff368be6592d3bf500c7",
        ),
        (
            192,
            "6e3a9b4ecba7af3a46e4f5c90fe02c99b5715144444b38049a42ac8313b30346",
        ),
        (
            200,
            "44cae5223d431caed4a9e32271d6abf17c3f2f4abac45fcdb48a99fcc6072a09",
        ),
    ];

    #[test]
    fn sha256_matches_known_answers_across_block_boundaries() {
        for (length, expected) in SHA256_ONE_SHOT {
            assert_eq!(sha256_hex(&pattern(*length)), *expected, "length {length}");
        }
    }

    const SHA1_ONE_SHOT: &[(usize, &str)] = &[
        (0, "da39a3ee5e6b4b0d3255bfef95601890afd80709"),
        (1, "5d1be7e9dda1ee8896be5b7e34a85ee16452a7b4"),
        (2, "7878ac025cfe0384191ff21ebb1627fd25f8a60c"),
        (3, "d2df16b976a43628c63ea246436fccf6d7635d09"),
        (31, "8a3cfab36a8e4e5510d5800dd0063725f29c614f"),
        (32, "4a9f58da1bac35a4d50e87dc0f9e49f3a740192f"),
        (55, "749bbefb28edc4638b28b2b9a9e03ab9a4032b90"),
        (56, "a5b6e9c29d201c774753ff8e7fb64931656f5e63"),
        (57, "eb0737bed5451790722b2df351829ce117e3d9dd"),
        (63, "d1a454409359fc372b4d22b3cea6488d6ba1be00"),
        (64, "39a0d8b645ad85f1f976731ed112ac9455e28b78"),
        (65, "d0c96e18890114a14716e9686528d2e3fdba8d9e"),
        (111, "b7b42d19ae6be209c36efe0c5dfe5bde4d306c43"),
        (119, "562ecf8a430f8e1056e3619bae33628e9a1d0a4e"),
        (120, "353f6d2bf0e91aa91b74a2e0b3f297510f7d825f"),
        (127, "bebc42d2d3d1e5fb8ad8895c2dcef2d68a6c279a"),
        (128, "0060f2a7e34b6e4d459f560197ef93243732a400"),
        (129, "3a16082d1bf09b604907ec6908b9893ca3e937c0"),
        (191, "90d881e31a36af55e527ceb7f011b051a6dd4ac2"),
        (192, "9d9112152625f518fae155f757471e6564167ac8"),
        (200, "9194d8145e556594766df1e7699e2dfda424d1ee"),
    ];

    #[test]
    fn sha1_matches_known_answers_across_block_boundaries() {
        // SHA-1 是 BT piece 校验与 info-hash 的唯一实现，错一块整包就废了。
        for (length, expected) in SHA1_ONE_SHOT {
            assert_eq!(sha1_hex(&pattern(*length)), *expected, "length {length}");
        }
    }

    const STREAMING_CASES: &[(usize, usize, &str)] = &[
        (
            0,
            1,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            1,
            1,
            "ca358758f6d27e6cf45272937977a748fd88391db679ceda7dc7bf1f005ee879",
        ),
        (
            55,
            1,
            "8aa994584139d128848eeebc4e815639ba5ab6e6e39574195a63ac4f14f7c43b",
        ),
        (
            55,
            7,
            "8aa994584139d128848eeebc4e815639ba5ab6e6e39574195a63ac4f14f7c43b",
        ),
        (
            56,
            1,
            "ad574708f75c044c9b85de64cb568ee7711ff4f36448c6242f053ba8f6cc2b63",
        ),
        (
            56,
            7,
            "ad574708f75c044c9b85de64cb568ee7711ff4f36448c6242f053ba8f6cc2b63",
        ),
        (
            57,
            1,
            "5b46e502092be01b1100193e089fdda95638c12e19a1d24f308eb2c3d3ae849d",
        ),
        (
            57,
            7,
            "5b46e502092be01b1100193e089fdda95638c12e19a1d24f308eb2c3d3ae849d",
        ),
        (
            63,
            1,
            "280ed3e8ff1df845b2e7dfe6ac6cee817bef20e783cc65abc41b818b4d2fe076",
        ),
        (
            63,
            63,
            "280ed3e8ff1df845b2e7dfe6ac6cee817bef20e783cc65abc41b818b4d2fe076",
        ),
        (
            64,
            1,
            "c6ab9724ade5b6a7a1edfffb12f3aa9181351355af8fd08c919952ad211339dd",
        ),
        (
            64,
            63,
            "c6ab9724ade5b6a7a1edfffb12f3aa9181351355af8fd08c919952ad211339dd",
        ),
        (
            64,
            64,
            "c6ab9724ade5b6a7a1edfffb12f3aa9181351355af8fd08c919952ad211339dd",
        ),
        (
            65,
            1,
            "788367c73c7ddf4c53f65e68cc0d943e6227ab55b0e78ba63ace822b1c6301c0",
        ),
        (
            65,
            64,
            "788367c73c7ddf4c53f65e68cc0d943e6227ab55b0e78ba63ace822b1c6301c0",
        ),
        (
            65,
            65,
            "788367c73c7ddf4c53f65e68cc0d943e6227ab55b0e78ba63ace822b1c6301c0",
        ),
        (
            119,
            1,
            "3d610547d68216dedf7435a4fb6260353911f6b3fd3f18805ddb8be285d726fe",
        ),
        (
            119,
            64,
            "3d610547d68216dedf7435a4fb6260353911f6b3fd3f18805ddb8be285d726fe",
        ),
        (
            119,
            65,
            "3d610547d68216dedf7435a4fb6260353911f6b3fd3f18805ddb8be285d726fe",
        ),
        (
            120,
            1,
            "1f80156a804cb7862ad113e8200e9d74499723e7c7854d5f48776d3148e09656",
        ),
        (
            120,
            64,
            "1f80156a804cb7862ad113e8200e9d74499723e7c7854d5f48776d3148e09656",
        ),
        (
            129,
            1,
            "81e89a7b2911aaa7795f9e3d4910cb47d6cd2b00d83b8399481527261a1a7519",
        ),
        (
            129,
            65,
            "81e89a7b2911aaa7795f9e3d4910cb47d6cd2b00d83b8399481527261a1a7519",
        ),
        (
            200,
            1,
            "44cae5223d431caed4a9e32271d6abf17c3f2f4abac45fcdb48a99fcc6072a09",
        ),
        (
            200,
            64,
            "44cae5223d431caed4a9e32271d6abf17c3f2f4abac45fcdb48a99fcc6072a09",
        ),
    ];

    #[test]
    fn sha256_hasher_matches_known_answers_for_every_chunk_boundary() {
        for (length, chunk, expected) in STREAMING_CASES {
            let data = pattern(*length);
            let mut hasher = Sha256Hasher::new();
            for piece in data.chunks(*chunk) {
                hasher.update(piece);
            }
            let digest = hasher
                .finish()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(digest, *expected, "length {length} chunk {chunk}");
        }
    }
}
