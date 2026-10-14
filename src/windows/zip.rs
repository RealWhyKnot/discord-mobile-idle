use std::io::Read;

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn end_of_directory(data: &[u8]) -> Option<usize> {
    let lowest = data.len().saturating_sub(22 + 65_535);
    (lowest..=data.len().checked_sub(22)?)
        .rev()
        .find(|&at| u32_at(data, at) == Some(0x0605_4b50))
}

pub fn extract(data: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let end = end_of_directory(data).ok_or("not a zip archive")?;
    let count = u16_at(data, end + 10).ok_or("truncated zip")? as usize;
    let mut at = u32_at(data, end + 16).ok_or("truncated zip")? as usize;
    for _ in 0..count {
        if u32_at(data, at) != Some(0x0201_4b50) {
            return Err("bad central directory".into());
        }
        let flags = u16_at(data, at + 8).ok_or("truncated zip")?;
        let method = u16_at(data, at + 10).ok_or("truncated zip")?;
        let crc = u32_at(data, at + 16).ok_or("truncated zip")?;
        let packed = u32_at(data, at + 20).ok_or("truncated zip")? as usize;
        let size = u32_at(data, at + 24).ok_or("truncated zip")? as usize;
        let name_len = u16_at(data, at + 28).ok_or("truncated zip")? as usize;
        let extra_len = u16_at(data, at + 30).ok_or("truncated zip")? as usize;
        let comment_len = u16_at(data, at + 32).ok_or("truncated zip")? as usize;
        let local = u32_at(data, at + 42).ok_or("truncated zip")? as usize;
        let entry = data.get(at + 46..at + 46 + name_len).ok_or("truncated zip")?;
        at += 46 + name_len + extra_len + comment_len;
        if entry != name.as_bytes() {
            continue;
        }
        if flags & 1 != 0 {
            return Err(format!("{} is encrypted", name));
        }
        if u32_at(data, local) != Some(0x0403_4b50) {
            return Err("bad local header".into());
        }
        let skip = u16_at(data, local + 26).ok_or("truncated zip")? as usize
            + u16_at(data, local + 28).ok_or("truncated zip")? as usize;
        let start = local + 30 + skip;
        let body = data.get(start..start + packed).ok_or("truncated zip")?;
        let out = match method {
            0 => body.to_vec(),
            8 => {
                let mut out = Vec::with_capacity(size);
                flate2::read::DeflateDecoder::new(body)
                    .read_to_end(&mut out)
                    .map_err(|e| format!("could not inflate {}: {}", name, e))?;
                out
            }
            other => return Err(format!("{} uses compression method {}", name, other)),
        };
        if out.len() != size {
            return Err(format!("{} unpacked to {} bytes, expected {}", name, out.len(), size));
        }
        let mut check = flate2::Crc::new();
        check.update(&out);
        if check.sum() != crc {
            return Err(format!("{} failed its CRC check", name));
        }
        return Ok(out);
    }
    Err(format!("there is no item named '{}' in the archive", name))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io::Write;

    pub fn build(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, body, deflate) in entries {
            let packed = if *deflate {
                let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(body).unwrap();
                encoder.finish().unwrap()
            } else {
                body.to_vec()
            };
            let mut crc = flate2::Crc::new();
            crc.update(body);
            let method: u16 = if *deflate { 8 } else { 0 };
            let offset = out.len() as u32;
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&[20, 0, 0, 0]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0, 0, 0, 0]);
            out.extend_from_slice(&crc.sum().to_le_bytes());
            out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&3u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&[1, 2, 3]);
            out.extend_from_slice(&packed);
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0, 0, 0, 0]);
            central.extend_from_slice(&crc.sum().to_le_bytes());
            central.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            central.extend_from_slice(&(body.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let start = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(b"hi");
        out
    }

    #[test]
    fn extracts_stored_and_deflated_entries() {
        let big = b"discord".repeat(1000);
        let archive = build(&[("README.md", b"read me", false), ("app.exe", &big, true)]);
        assert_eq!(extract(&archive, "README.md").unwrap(), b"read me");
        assert_eq!(extract(&archive, "app.exe").unwrap(), big);
    }

    #[test]
    fn reports_a_missing_entry_and_garbage() {
        let archive = build(&[("README.md", b"read me", false)]);
        assert!(extract(&archive, "app.exe").unwrap_err().contains("no item named"));
        assert!(extract(b"not a zip", "app.exe").is_err());
        assert!(extract(&[], "app.exe").is_err());
        assert!(extract(&archive[..archive.len() - 30], "README.md").is_err());
    }

    #[test]
    fn catches_a_corrupted_body() {
        let mut archive = build(&[("app.exe", b"0123456789", false)]);
        let at = archive.windows(10).position(|w| w == b"0123456789").unwrap();
        archive[at] = b'X';
        assert!(extract(&archive, "app.exe").unwrap_err().contains("CRC"));
    }
}
