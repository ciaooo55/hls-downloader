//! Central-directory preview only; never decompresses or executes an entry.
//! Layout: PKWARE APPNOTE 6.3.10, sections 4.3.12–4.3.16 and 4.5.3.
use crate::{TaskSpec, ZipEntryPreview, ZipPreview};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

const TAIL: u64 = 65535 + 22 + 20;
const MAX_METADATA: u64 = 4 * 1024 * 1024;
const MAX_ENTRIES: u64 = 10_000;
const CORRUPT: &str = "ZIP 目录损坏、加密或格式不受支持";

struct Source {
    file: Option<File>,
    spec: Option<TaskSpec>,
    total: u64,
    tail: Vec<u8>,
    read: u64,
    modified: Option<std::time::SystemTime>,
}

impl Source {
    fn open(path: &str, spec: Option<TaskSpec>) -> Result<Self, String> {
        if let Some(mut spec) = spec {
            let response = crate::http_engine::read_metadata(
                &spec,
                Some(&format!("bytes=-{TAIL}")),
                TAIL,
                None,
            )?;
            let (start, end, total) = response.range.ok_or(CORRUPT)?;
            let total = total.ok_or(CORRUPT)?;
            if end.checked_add(1) != Some(total)
                || start != total.saturating_sub(TAIL)
                || response.body.len() as u64 != total - start
            {
                return Err(CORRUPT.into());
            }
            spec.etag = response.etag;
            spec.last_modified = response.last_modified;
            spec.url = response.final_url;
            let read = response.body.len() as u64;
            Ok(Self {
                file: None,
                spec: Some(spec),
                total,
                tail: response.body,
                read,
                modified: None,
            })
        } else {
            let mut file = File::open(path).map_err(|_| "无法打开本地 ZIP")?;
            let metadata = file.metadata().map_err(|_| CORRUPT)?;
            let total = metadata.len();
            let modified = metadata.modified().ok();
            let length = total.min(TAIL);
            file.seek(SeekFrom::Start(total - length))
                .map_err(|_| CORRUPT)?;
            let mut tail = vec![0; length as usize];
            file.read_exact(&mut tail).map_err(|_| CORRUPT)?;
            Ok(Self {
                file: Some(file),
                spec: None,
                total,
                tail,
                read: length,
                modified,
            })
        }
    }

    fn bytes(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, String> {
        let end = offset.checked_add(length).ok_or(CORRUPT)?;
        if end > self.total {
            return Err(CORRUPT.into());
        }
        if length == 0 {
            return Ok(Vec::new());
        }
        let tail_start = self.total - self.tail.len() as u64;
        if offset >= tail_start {
            return Ok(
                self.tail[(offset - tail_start) as usize..(end - tail_start) as usize].to_vec(),
            );
        }
        if length > MAX_METADATA || self.read.saturating_add(length) > MAX_METADATA {
            return Err("ZIP 元数据超过 4 MiB 上限".into());
        }
        let bytes = if let Some(file) = &mut self.file {
            file.seek(SeekFrom::Start(offset)).map_err(|_| CORRUPT)?;
            let mut data = vec![0; length as usize];
            file.read_exact(&mut data).map_err(|_| CORRUPT)?;
            data
        } else {
            let spec = self.spec.as_ref().ok_or(CORRUPT)?;
            if (spec.etag.is_empty() || spec.etag.starts_with("W/"))
                && spec.last_modified.is_empty()
            {
                return Err("远程 ZIP 没有可靠的资源标识，请完整下载后预览".into());
            }
            let response = crate::http_engine::read_metadata(
                spec,
                Some(&format!("bytes={offset}-{}", end - 1)),
                length,
                None,
            )?;
            if response.range != Some((offset, end - 1, Some(self.total)))
                || response.body.len() as u64 != length
            {
                return Err("远程 ZIP 范围或资源长度已变化".into());
            }
            response.body
        };
        self.read += length;
        Ok(bytes)
    }
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, String> {
    Ok(u16::from_le_bytes(
        b.get(at..at + 2)
            .ok_or(CORRUPT)?
            .try_into()
            .map_err(|_| CORRUPT)?,
    ))
}
fn u32_at(b: &[u8], at: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        b.get(at..at + 4)
            .ok_or(CORRUPT)?
            .try_into()
            .map_err(|_| CORRUPT)?,
    ))
}
fn u64_at(b: &[u8], at: usize) -> Result<u64, String> {
    Ok(u64::from_le_bytes(
        b.get(at..at + 8)
            .ok_or(CORRUPT)?
            .try_into()
            .map_err(|_| CORRUPT)?,
    ))
}

pub(crate) fn preview(path: &str, spec: Option<TaskSpec>) -> Result<ZipPreview, String> {
    let mut source = Source::open(path, spec)?;
    let tail = source.tail.clone();
    let eocd = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&at| {
            tail.get(at..at + 4) == Some(b"PK\x05\x06")
                && u16_at(&tail, at + 20).is_ok_and(|n| at + 22 + n as usize == tail.len())
        })
        .ok_or(CORRUPT)?;
    let eocd_offset = source.total - tail.len() as u64 + eocd as u64;
    if u16_at(&tail, eocd + 4)? != 0 || u16_at(&tail, eocd + 6)? != 0 {
        return Err("不支持分卷 ZIP 预览".into());
    }
    let mut count = u16_at(&tail, eocd + 10)? as u64;
    if u16_at(&tail, eocd + 8)? as u64 != count {
        return Err(CORRUPT.into());
    }
    let mut size = u32_at(&tail, eocd + 12)? as u64;
    let mut offset = u32_at(&tail, eocd + 16)? as u64;
    let zip64 = count == 0xffff
        || size == 0xffffffff
        || offset == 0xffffffff
        || eocd >= 20 && tail.get(eocd - 20..eocd - 16) == Some(b"PK\x06\x07");
    let mut directory_end = eocd_offset;
    if zip64 {
        let locator = source.bytes(eocd_offset.checked_sub(20).ok_or(CORRUPT)?, 20)?;
        if locator.get(..4) != Some(b"PK\x06\x07")
            || u32_at(&locator, 4)? != 0
            || u32_at(&locator, 16)? != 1
        {
            return Err("不支持分卷或损坏的 ZIP64".into());
        }
        let record_offset = u64_at(&locator, 8)?;
        let record = source.bytes(record_offset, 56)?;
        if record.get(..4) != Some(b"PK\x06\x06")
            || u64_at(&record, 4)? < 44
            || record_offset
                .checked_add(12)
                .and_then(|n| n.checked_add(u64_at(&record, 4).ok()?))
                != eocd_offset.checked_sub(20)
            || u32_at(&record, 16)? != 0
            || u32_at(&record, 20)? != 0
            || u64_at(&record, 24)? != u64_at(&record, 32)?
        {
            return Err(CORRUPT.into());
        }
        count = u64_at(&record, 32)?;
        size = u64_at(&record, 40)?;
        offset = u64_at(&record, 48)?;
        directory_end = record_offset;
    }
    if count > MAX_ENTRIES {
        return Err("ZIP 条目超过 10000 项预览上限".into());
    }
    if size > MAX_METADATA {
        return Err("ZIP 目录超过 4 MiB 预览上限".into());
    }
    if offset.checked_add(size).is_none_or(|n| n > directory_end) || (count == 0 && size != 0) {
        return Err(CORRUPT.into());
    }
    let directory = source.bytes(offset, size)?;
    let entries = parse_directory(&directory, count)?;
    if let Some(file) = &source.file {
        let current = file.metadata().map_err(|_| CORRUPT)?;
        if current.len() != source.total
            || source
                .modified
                .is_some_and(|stamp| current.modified().ok() != Some(stamp))
        {
            return Err("预览期间 ZIP 文件已变化".into());
        }
    }
    if serde_json::to_vec(&entries).map_err(|_| CORRUPT)?.len() > 3 * 1024 * 1024 {
        return Err("ZIP 目录展示数据超过 3 MiB 上限".into());
    }
    Ok(ZipPreview {
        entries,
        archive_bytes: source.total,
        metadata_bytes_read: source.read,
        zip64,
    })
}

fn parse_directory(bytes: &[u8], count: u64) -> Result<Vec<ZipEntryPreview>, String> {
    let mut at = 0;
    let mut entries = Vec::new();
    for _ in 0..count {
        if bytes.get(at..at + 4) != Some(b"PK\x01\x02") {
            return Err(CORRUPT.into());
        }
        let flags = u16_at(bytes, at + 8)?;
        if flags & (1 << 13) != 0 {
            return Err("不支持加密 ZIP 目录".into());
        }
        let method = u16_at(bytes, at + 10)?;
        let mut compressed = u32_at(bytes, at + 20)? as u64;
        let mut uncompressed = u32_at(bytes, at + 24)? as u64;
        let name_len = u16_at(bytes, at + 28)? as usize;
        let extra_len = u16_at(bytes, at + 30)? as usize;
        let comment_len = u16_at(bytes, at + 32)? as usize;
        let disk = u16_at(bytes, at + 34)?;
        let local_offset = u32_at(bytes, at + 42)?;
        let name = bytes.get(at + 46..at + 46 + name_len).ok_or(CORRUPT)?;
        let extra = bytes
            .get(at + 46 + name_len..at + 46 + name_len + extra_len)
            .ok_or(CORRUPT)?;
        let mut extras = 0;
        let mut zip64 = None;
        let mut unicode_name = None;
        while extras < extra.len() {
            let tag = u16_at(extra, extras)?;
            let length = u16_at(extra, extras + 2)? as usize;
            let data = extra.get(extras + 4..extras + 4 + length).ok_or(CORRUPT)?;
            if tag == 1 {
                zip64 = Some(data);
            }
            if tag == 0x7075 && data.len() >= 5 && data[0] == 1 && u32_at(data, 1)? == crc32(name) {
                unicode_name = Some(
                    std::str::from_utf8(&data[5..])
                        .map_err(|_| CORRUPT)?
                        .to_string(),
                );
            }
            extras += 4 + length;
        }
        if uncompressed == 0xffffffff
            || compressed == 0xffffffff
            || local_offset == 0xffffffff
            || disk == 0xffff
        {
            let data = zip64.ok_or(CORRUPT)?;
            let mut pos = 0;
            if uncompressed == 0xffffffff {
                uncompressed = u64_at(data, pos)?;
                pos += 8;
            }
            if compressed == 0xffffffff {
                compressed = u64_at(data, pos)?;
                pos += 8;
            }
            if local_offset == 0xffffffff {
                u64_at(data, pos)?;
                pos += 8;
            }
            if disk == 0xffff && u32_at(data, pos)? != 0 {
                return Err("不支持分卷 ZIP".into());
            }
        }
        if disk != 0 && disk != 0xffff {
            return Err("不支持分卷 ZIP".into());
        }
        let path = if flags & (1 << 11) != 0 {
            String::from_utf8(name.to_vec()).map_err(|_| CORRUPT)?
        } else {
            unicode_name.unwrap_or_else(|| decode_cp437(name))
        };
        entries.push(ZipEntryPreview {
            path,
            compressed_bytes: compressed,
            uncompressed_bytes: uncompressed,
            method,
            encrypted: flags & 1 != 0,
        });
        at = at
            .checked_add(46 + name_len + extra_len + comment_len)
            .ok_or(CORRUPT)?;
        if at > bytes.len() {
            return Err(CORRUPT.into());
        }
    }
    // The optional central-directory digital signature is metadata, not an entry.
    if at != bytes.len()
        && !(bytes.get(at..at + 4) == Some(b"PK\x05\x05")
            && at + 6 + u16_at(bytes, at + 4)? as usize == bytes.len())
    {
        return Err(CORRUPT.into());
    }
    Ok(entries)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
        }
    }
    !crc
}

fn decode_cp437(name: &[u8]) -> String {
    const HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■ ";
    let high: Vec<char> = HIGH.chars().collect();
    name.iter()
        .map(|&b| {
            if b < 128 {
                b as char
            } else {
                high[(b - 128) as usize]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_remote_directory_does_not_issue_a_range_request() {
        let mut source = Source {
            file: None,
            spec: Some(TaskSpec::default()),
            total: TAIL + 22,
            tail: vec![0; TAIL as usize],
            read: TAIL,
            modified: None,
        };
        assert!(source.bytes(0, 0).unwrap().is_empty());
        assert_eq!(source.read, TAIL);
        assert!(source.bytes(source.total + 1, 0).is_err());
    }

    #[test]
    fn directory_preserves_utf8_encryption_and_zip64_sizes() {
        let name = "目录/文件.txt".as_bytes();
        let mut directory = vec![0; 46];
        directory[..4].copy_from_slice(b"PK\x01\x02");
        directory[8..10].copy_from_slice(&(0x801u16).to_le_bytes());
        directory[10..12].copy_from_slice(&8u16.to_le_bytes());
        directory[20..28].fill(0xff);
        directory[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
        directory[30..32].copy_from_slice(&20u16.to_le_bytes());
        directory.extend_from_slice(name);
        directory.extend_from_slice(&1u16.to_le_bytes());
        directory.extend_from_slice(&16u16.to_le_bytes());
        directory.extend_from_slice(&9_000_000_000u64.to_le_bytes());
        directory.extend_from_slice(&4_000_000_000u64.to_le_bytes());
        let entries = parse_directory(&directory, 1).unwrap();
        assert_eq!(entries[0].path, "目录/文件.txt");
        assert!(entries[0].encrypted);
        assert_eq!(entries[0].uncompressed_bytes, 9_000_000_000);
        assert_eq!(entries[0].compressed_bytes, 4_000_000_000);
        directory.pop();
        assert!(parse_directory(&directory, 1).is_err());
        assert!(parse_directory(&[], 1).is_err());
    }
}
