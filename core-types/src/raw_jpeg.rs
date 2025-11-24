use std::fs;
use std::io;
use std::path::Path;

/// Returns the largest embedded JPEG (by byte length) found inside the file.
pub fn find_embedded_jpeg(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let data = fs::read(path)?;
    Ok(find_embedded_jpeg_in_data(&data))
}

fn find_embedded_jpeg_in_data(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 4 {
        return None;
    }
    let mut best: Option<(Vec<u8>, usize)> = None;
    let mut idx = 0;
    while idx + 1 < data.len() {
        if data[idx] == 0xFF && data[idx + 1] == 0xD8 {
            if let Some(end) = find_jpeg_end(data, idx + 2) {
                let len = end - idx;
                if best.as_ref().map_or(true, |(_, best_len)| len > *best_len) {
                    best = Some((data[idx..end].to_vec(), len));
                }
                idx = end;
                continue;
            } else {
                break;
            }
        }
        idx += 1;
    }
    best.map(|(bytes, _)| bytes)
}

fn find_jpeg_end(data: &[u8], mut idx: usize) -> Option<usize> {
    while idx + 1 < data.len() {
        if data[idx] == 0xFF && data[idx + 1] == 0xD9 {
            return Some(idx + 2);
        }
        idx += 1;
    }
    None
}

/// Extracts the APP1 Exif payload from a JPEG buffer, excluding the "Exif\0\0" header.
pub fn extract_exif_segment(jpeg_bytes: &[u8]) -> Option<Vec<u8>> {
    if jpeg_bytes.len() < 4 {
        return None;
    }
    if jpeg_bytes[0] != 0xFF || jpeg_bytes[1] != 0xD8 {
        return None;
    }
    let mut idx = 2;
    while idx + 3 < jpeg_bytes.len() {
        if jpeg_bytes[idx] != 0xFF {
            idx += 1;
            continue;
        }
        let marker = jpeg_bytes[idx + 1];
        idx += 2;
        if marker == 0xD9 || marker == 0xDA {
            break;
        }
        if idx + 2 > jpeg_bytes.len() {
            break;
        }
        let len = u16::from_be_bytes([jpeg_bytes[idx], jpeg_bytes[idx + 1]]) as usize;
        if len < 2 || idx + len > jpeg_bytes.len() {
            break;
        }
        let payload = &jpeg_bytes[idx + 2..idx + len];
        if marker == 0xE1 && payload.starts_with(b"Exif\0\0") {
            return Some(payload[6..].to_vec());
        }
        idx += len;
    }
    None
}
