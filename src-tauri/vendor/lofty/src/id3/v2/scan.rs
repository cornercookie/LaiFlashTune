//! Size-bounded ID3v2 text parsing used only by the library scanner.

use super::frame::content::parse_content;
use super::frame::header::{parse_header, parse_v2_header};
use super::tag::ID3v2Tag;
use super::{Frame, FrameFlags, FrameValue, ID3v2Version};
use crate::error::Result;
use flate2::read::ZlibDecoder;
use std::io::{self, BufReader, Read, Seek, SeekFrom};

const FIELD_BYTES: usize = 4 * 1024;

fn invalid_tag() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid ID3v2 scan header or frame")
}

fn synchsafe_size(bytes: [u8; 4]) -> io::Result<usize> {
    if bytes.iter().any(|byte| byte & 0x80 != 0) {
        return Err(invalid_tag());
    }
    Ok(bytes.into_iter().fold(0usize, |len, byte| (len << 7) | usize::from(byte)))
}

fn skip_exact(input: &mut impl Read, length: usize) -> io::Result<()> {
    let length = length as u64;
    if io::copy(&mut input.take(length), &mut io::sink())? != length {
        return Err(invalid_tag());
    }
    Ok(())
}

struct Unsynchronised<R> {
    input: R,
    previous_ff: bool,
}

impl<R: Read> Read for Unsynchronised<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut count = 0;
        let mut byte = [0];
        while count < buffer.len() && self.input.read(&mut byte)? != 0 {
            let skip = self.previous_ff && byte[0] == 0;
            self.previous_ff = byte[0] == 0xff;
            if !skip {
                buffer[count] = byte[0];
                count += 1;
            }
        }
        Ok(count)
    }
}

fn undo_frame_unsync(bytes: &mut [u8]) -> usize {
    let mut write = 0;
    let mut previous_ff = false;
    for read in 0..bytes.len() {
        let byte = bytes[read];
        if !previous_ff || byte != 0 {
            bytes[write] = byte;
            write += 1;
        }
        previous_ff = byte == 0xff;
    }
    write
}

fn describe_value<'a>(id: &'a str, value: &'a FrameValue) -> Option<(&'a str, &'a str)> {
    match value {
        FrameValue::Text { value, .. } | FrameValue::URL(value) => Some((id, value)),
        FrameValue::Comment(value) | FrameValue::UnSyncText(value) => Some((id, &value.content)),
        FrameValue::UserText(value) | FrameValue::UserURL(value) => {
            Some((&value.description, &value.content))
        }
        _ => None,
    }
}

fn frame_text_cost(id: &str, value: &FrameValue) -> Option<usize> {
    let (key, text) = describe_value(id, value)?;
    if key.is_empty()
        || key.len() > FIELD_BYTES
        || text.len() > FIELD_BYTES
        || key.eq_ignore_ascii_case("METADATA_BLOCK_PICTURE")
        || key.eq_ignore_ascii_case("COVERART")
    {
        return None;
    }
    let cost = text.split(['\0', '/']).map(|item| key.len() + item.len()).sum();
    Some(cost)
}

fn read_scan_frames(
    input: &mut impl Read,
    version: ID3v2Version,
    tag_flags: u8,
    remaining: &mut usize,
) -> Result<ID3v2Tag> {
    let version4 = version == ID3v2Version::V4;
    if version != ID3v2Version::V2 && tag_flags & 0x40 != 0 {
        let mut length = [0; 4];
        input.read_exact(&mut length)?;
        let extended = if version4 {
            synchsafe_size(length)?.checked_sub(4).ok_or_else(invalid_tag)?
        } else {
            u32::from_be_bytes(length) as usize
        };
        if extended < if version4 { 2 } else { 6 } {
            return Err(invalid_tag().into());
        }
        skip_exact(input, extended)?;
    }

    let mut tag = ID3v2Tag::default();
    tag.original_version = version;
    let header_len = if version == ID3v2Version::V2 { 6 } else { 10 };
    let mut encoded = [0; FIELD_BYTES];
    let mut decoded = [0; FIELD_BYTES + 1];
    loop {
        let mut header = [0; 10];
        if input.read(&mut header[..1])? == 0 || header[0] == 0 {
            break;
        }
        input.read_exact(&mut header[1..header_len])?;
        if version4 {
            synchsafe_size(header[4..8].try_into().unwrap())?;
        }
        let mut header_data = &header[..header_len];
        let (id, size, _) = if version == ID3v2Version::V2 {
            parse_v2_header(&mut header_data)?
        } else {
            parse_header(&mut header_data, version4)?
        }.ok_or_else(invalid_tag)?;

        let flags = if header_len == 6 { 0 } else { header[9] };
        let supported_text = id.as_str().starts_with(['T', 'W'])
            || matches!(id.as_str(), "COMM" | "USLT");
        let encrypted = (version4 && flags & 0x04 != 0) || (!version4 && flags & 0x40 != 0);
        let size = size as usize;
        if !supported_text || encrypted || size > FIELD_BYTES || *remaining == 0 {
            skip_exact(input, size)?;
            continue;
        }
        input.read_exact(&mut encoded[..size])?;

        let length = if version4 && (tag_flags & 0x80 != 0 || flags & 0x02 != 0) {
            undo_frame_unsync(&mut encoded[..size])
        } else {
            size
        };
        let mut value_bytes = &encoded[..length];
        let compressed = if version4 { flags & 0x08 != 0 } else { flags & 0x80 != 0 };
        let grouped = if version4 { flags & 0x40 != 0 } else { flags & 0x20 != 0 };
        let mut decoded_length = None;
        if version4 && grouped {
            value_bytes = value_bytes.get(1..).ok_or_else(invalid_tag)?;
        }
        if (version4 && flags & 0x01 != 0) || (!version4 && compressed) {
            let mut expected = [0; 4];
            value_bytes.read_exact(&mut expected)?;
            decoded_length = Some(if version4 {
                synchsafe_size(expected)?
            } else {
                u32::from_be_bytes(expected) as usize
            });
        }
        if !version4 && grouped {
            value_bytes = value_bytes.get(1..).ok_or_else(invalid_tag)?;
        }
        if compressed {
            let expected = decoded_length.ok_or_else(invalid_tag)?;
            if expected > FIELD_BYTES {
                continue;
            }
            let mut inflate = ZlibDecoder::new(value_bytes);
            let mut count = 0;
            while count < decoded.len() {
                let read = inflate.read(&mut decoded[count..])?;
                if read == 0 {
                    break;
                }
                count += read;
            }
            if count > FIELD_BYTES {
                continue;
            }
            if count != expected {
                return Err(invalid_tag().into());
            }
            value_bytes = &decoded[..count];
        }

        if let Some(value) = parse_content(&mut value_bytes, id.as_str(), version)? {
            if let Some(cost) = frame_text_cost(id.as_str(), &value) {
                if cost <= *remaining {
                    *remaining -= cost;
                    drop(tag.insert(Frame { id, value, flags: FrameFlags::default() }));
                }
            }
        }
    }
    Ok(tag)
}

pub(crate) fn read_id3v2_prefixes<R: Read + Seek>(
    file: &mut R,
    end: u64,
    budget: &mut usize,
) -> Result<Option<ID3v2Tag>> {
    let mut tags: Option<ID3v2Tag> = None;
    loop {
        let start = file.stream_position()?;
        if end.saturating_sub(start) < 3 {
            break;
        }
        let mut prefix = [0; 3];
        file.read_exact(&mut prefix)?;
        file.seek(SeekFrom::Start(start))?;
        if &prefix != b"ID3" {
            break;
        }
        let next = read_id3v2_for_scan(file, end, budget)?;
        if let Some(previous) = &mut tags {
            for frame in next {
                drop(previous.insert(frame));
            }
        } else {
            tags = Some(next);
        }
    }
    Ok(tags)
}

pub(crate) fn read_id3v2_for_scan<R: Read + Seek>(
    file: &mut R,
    parent_end: u64,
    remaining: &mut usize,
) -> Result<ID3v2Tag> {
    let mut header = [0; 10];
    let start = file.stream_position()?;
    if parent_end.checked_sub(start).is_none_or(|available| available < 10) {
        return Err(invalid_tag().into());
    }
    file.read_exact(&mut header)?;
    if &header[..3] != b"ID3" || header[4] == 0xff {
        return Err(invalid_tag().into());
    }
    let version = match header[3] {
        2 => ID3v2Version::V2,
        3 => ID3v2Version::V3,
        4 => ID3v2Version::V4,
        _ => return Err(invalid_tag().into()),
    };
    let body_length = synchsafe_size(header[6..].try_into().unwrap())? as u64;
    let body_start = file.stream_position()?;
    let footer_length = if version == ID3v2Version::V4 && header[5] & 0x10 != 0 { 10 } else { 0 };
    let body_end = body_start.checked_add(body_length)
        .filter(|end| end.checked_add(footer_length).is_some_and(|end| end <= parent_end))
        .ok_or_else(invalid_tag)?;

    let result = if version == ID3v2Version::V2 && header[5] & 0x40 != 0 {
        ID3v2Tag::default()
    } else {
        let mut body = BufReader::with_capacity(8 * 1024, (&mut *file).take(body_length));
        if version != ID3v2Version::V4 && header[5] & 0x80 != 0 {
            let mut input = Unsynchronised { input: body, previous_ff: false };
            read_scan_frames(&mut input, version, header[5], remaining)?
        } else {
            read_scan_frames(&mut body, version, header[5], remaining)?
        }
    };

    file.seek(SeekFrom::Start(body_end))?;
    if footer_length > 0 {
        let mut footer = [0; 10];
        file.read_exact(&mut footer)?;
        if &footer[..3] != b"3DI" || footer[3..] != header[3..] {
            return Err(invalid_tag().into());
        }
    }
    Ok(result)
}
