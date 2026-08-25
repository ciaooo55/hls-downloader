//! Whole-file digest check before a download is published.

use crate::crypto_lite::{sha1_hex, Sha256Hasher};
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    Md5,
    Sha1,
    Sha256,
}

impl Algorithm {
    pub fn label(self) -> &'static str {
        match self {
            Self::Md5 => "MD5",
            Self::Sha1 => "SHA-1",
            Self::Sha256 => "SHA-256",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationResult {
    pub algorithm: String,
    pub expected: String,
    pub actual: String,
    pub verified: bool,
}

pub fn parse_checksum(value: &str) -> Option<(Algorithm, String)> {
    let text = value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_ascii_lowercase();
    if text.is_empty() {
        return None;
    }
    let (algo, rest) = if let Some(rest) = text.strip_prefix("sha256:") {
        (Algorithm::Sha256, rest)
    } else if let Some(rest) = text.strip_prefix("sha-256:") {
        (Algorithm::Sha256, rest)
    } else if let Some(rest) = text.strip_prefix("sha1:") {
        (Algorithm::Sha1, rest)
    } else if let Some(rest) = text.strip_prefix("sha-1:") {
        (Algorithm::Sha1, rest)
    } else if let Some(rest) = text.strip_prefix("md5:") {
        (Algorithm::Md5, rest)
    } else if text.len() == 64 && is_hex(&text) {
        (Algorithm::Sha256, text.as_str())
    } else if text.len() == 40 && is_hex(&text) {
        (Algorithm::Sha1, text.as_str())
    } else if text.len() == 32 && is_hex(&text) {
        (Algorithm::Md5, text.as_str())
    } else {
        return None;
    };
    let digest = rest.replace(':', "").replace(' ', "");
    if !is_hex(&digest) {
        return None;
    }
    Some((algo, digest))
}

pub fn hash_file(path: &Path, algorithm: Algorithm) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| format!("open for checksum: {error}"))?;
    let mut buffer = [0u8; 64 * 1024];
    match algorithm {
        Algorithm::Sha256 => {
            let mut hasher = Sha256Hasher::new();
            loop {
                let count = file
                    .read(&mut buffer)
                    .map_err(|error| format!("read for checksum: {error}"))?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
            }
            Ok(hasher
                .finish()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect())
        }
        Algorithm::Sha1 => {
            let mut data = Vec::new();
            file.read_to_end(&mut data)
                .map_err(|error| format!("read for checksum: {error}"))?;
            Ok(sha1_hex(&data))
        }
        Algorithm::Md5 => {
            let mut hasher = Md5Hasher::new();
            loop {
                let count = file
                    .read(&mut buffer)
                    .map_err(|error| format!("read for checksum: {error}"))?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
            }
            Ok(hasher.finish_hex())
        }
    }
}

pub fn verify_file(path: &Path, expected: &str) -> Result<(), String> {
    let Some(result) = verify_file_result(path, expected)? else {
        return Ok(());
    };
    if result.verified {
        Ok(())
    } else {
        Err(format!(
            "checksum mismatch: expected {}, got {}",
            result.expected, result.actual
        ))
    }
}

pub fn verify_file_result(
    path: &Path,
    expected: &str,
) -> Result<Option<VerificationResult>, String> {
    let Some((algorithm, want)) = parse_checksum(expected) else {
        return Ok(None);
    };
    let actual = hash_file(path, algorithm)?;
    Ok(Some(VerificationResult {
        algorithm: algorithm.label().into(),
        verified: actual == want,
        expected: want,
        actual,
    }))
}

fn is_hex(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

struct Md5Hasher {
    state: [u32; 4],
    buffer: [u8; 64],
    filled: usize,
    total_bytes: u64,
}

impl Md5Hasher {
    fn new() -> Self {
        Self {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            buffer: [0; 64],
            filled: 0,
            total_bytes: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_bytes += data.len() as u64;
        if self.filled > 0 {
            let take = (64 - self.filled).min(data.len());
            self.buffer[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == 64 {
                md5_compress(&mut self.state, &self.buffer);
                self.filled = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            md5_compress(&mut self.state, &block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.filled = data.len();
        }
    }

    fn finish_hex(mut self) -> String {
        let bit_len = self.total_bytes * 8;
        self.buffer[self.filled] = 0x80;
        self.filled += 1;
        if self.filled > 56 {
            for slot in self.buffer.iter_mut().skip(self.filled) {
                *slot = 0;
            }
            md5_compress(&mut self.state, &self.buffer);
            self.filled = 0;
        }
        for slot in self.buffer.iter_mut().take(56).skip(self.filled) {
            *slot = 0;
        }
        self.buffer[56..64].copy_from_slice(&bit_len.to_le_bytes());
        md5_compress(&mut self.state, &self.buffer);
        let mut out = [0u8; 16];
        for (index, word) in self.state.iter().enumerate() {
            out[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        out.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

fn md5_compress(state: &mut [u32; 4], chunk: &[u8; 64]) {
    let mut m = [0u32; 16];
    for (index, part) in chunk.chunks_exact(4).enumerate() {
        m[index] = u32::from_le_bytes(part.try_into().unwrap());
    }
    let (mut a, mut b, mut c, mut d) = (state[0], state[1], state[2], state[3]);
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
    for i in 0..64 {
        let (f, g) = match i {
            0..=15 => ((b & c) | ((!b) & d), i),
            16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
            32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | (!d)), (7 * i) % 16),
        };
        let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(f.rotate_left(S[i]));
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_prefixed_and_bare_digests() {
        assert_eq!(
            parse_checksum(
                "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            )
            .unwrap()
            .0,
            Algorithm::Sha256
        );
        assert_eq!(
            parse_checksum("d41d8cd98f00b204e9800998ecf8427e")
                .unwrap()
                .0,
            Algorithm::Md5
        );
    }

    #[test]
    fn verifies_empty_sha256_file() {
        let dir = std::env::temp_dir().join("v6-checksum-empty");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("empty.bin");
        std::fs::write(&path, b"").unwrap();
        verify_file(
            &path,
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn md5_empty_matches_known_vector() {
        let dir = std::env::temp_dir().join("v6-checksum-md5");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("empty.bin");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"").unwrap();
        assert_eq!(
            hash_file(&path, Algorithm::Md5).unwrap(),
            "d41d8cd98f00b204e9800998ecf8427e"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mismatch_fails_closed() {
        let dir = std::env::temp_dir().join("v6-checksum-bad");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("a.bin");
        std::fs::write(&path, b"abc").unwrap();
        let error = verify_file(&path, "md5:d41d8cd98f00b204e9800998ecf8427e").unwrap_err();
        assert!(error.contains("mismatch"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
