use t3_contracts::assets::AssetImageDimensions;
pub const HEADER_BYTES: u64 = 256 * 1024;
fn u16_at(b: &[u8], i: usize, le: bool) -> Option<u16> {
    let bytes = b.get(i..i + 2)?.try_into().ok()?;
    Some(if le {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    })
}
fn u32_at(b: &[u8], i: usize, le: bool) -> Option<u32> {
    let bytes = b.get(i..i + 4)?.try_into().ok()?;
    Some(if le {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    })
}
pub fn dimensions(b: &[u8]) -> Option<AssetImageDimensions> {
    let (width, height) =
        if b.len() >= 24 && b.starts_with(b"\x89PNG") && b.get(12..16) == Some(b"IHDR") {
            (u32_at(b, 16, false)?, u32_at(b, 20, false)?)
        } else if b.len() >= 10 && b.starts_with(b"GIF8") {
            (u16_at(b, 6, true)? as u32, u16_at(b, 8, true)? as u32)
        } else if b.len() >= 16 && b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
            match b.get(12..16)? {
                b"VP8 " => (
                    u16_at(b, 26, true)? as u32 & 0x3fff,
                    u16_at(b, 28, true)? as u32 & 0x3fff,
                ),
                b"VP8L" => {
                    let p = u32_at(b, 21, true)?;
                    ((p & 0x3fff) + 1, ((p >> 14) & 0x3fff) + 1)
                }
                b"VP8X" => {
                    let b = b.get(..30)?;
                    (
                        (b[24] as u32 | ((b[25] as u32) << 8) | ((b[26] as u32) << 16)) + 1,
                        (b[27] as u32 | ((b[28] as u32) << 8) | ((b[29] as u32) << 16)) + 1,
                    )
                }
                _ => return None,
            }
        } else {
            jpeg(b)?
        };
    (width > 0 && height > 0).then_some(AssetImageDimensions {
        width: width as u64,
        height: height as u64,
    })
}
fn jpeg(b: &[u8]) -> Option<(u32, u32)> {
    if !b.starts_with(b"\xff\xd8") {
        return None;
    }
    let mut offset = 2;
    let mut rotated = false;
    while offset + 9 <= b.len() {
        if b[offset] != 0xff {
            return None;
        }
        let marker = b[offset + 1];
        if marker == 0xff {
            offset += 1;
            continue;
        }
        if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
            let h = u16_at(b, offset + 5, false)? as u32;
            let w = u16_at(b, offset + 7, false)? as u32;
            return Some(if rotated { (h, w) } else { (w, h) });
        }
        if [0xd9, 0xda].contains(&marker) {
            return None;
        }
        if marker == 1 || (0xd0..=0xd7).contains(&marker) {
            offset += 2;
            continue;
        }
        let length = u16_at(b, offset + 2, false)? as usize;
        if marker == 0xe1 && !rotated {
            rotated =
                exif_rotated(b, offset + 4, (offset + 2 + length).min(b.len())).unwrap_or(false);
        }
        offset += 2 + length;
    }
    None
}
fn exif_rotated(b: &[u8], start: usize, end: usize) -> Option<bool> {
    if end.checked_sub(start)? < 14 || b.get(start..start + 4)? != b"Exif" {
        return None;
    }
    let tiff = start + 6;
    let le = match b.get(tiff..tiff + 2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let ifd = tiff.checked_add(u32_at(b, tiff + 4, le)? as usize)?;
    if ifd + 2 > end {
        return None;
    }
    for i in 0..u16_at(b, ifd, le)? as usize {
        let entry = ifd + 2 + i * 12;
        if entry + 12 > end {
            return None;
        }
        if u16_at(b, entry, le)? == 0x112 {
            return Some((5..=8).contains(&u16_at(b, entry + 8, le)?));
        }
    }
    None
}
