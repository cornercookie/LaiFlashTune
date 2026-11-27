//! Library scanning for WAVE and AIFF with local chunk and text bounds.

use super::aiff::{tag::{AIFFTextChunks, Comment}, AiffFile};
use super::wav::{tag::RIFFInfoList, WavFile};
use crate::error::Result;
use crate::id3::v2::{scan::read_id3v2_for_scan, tag::ID3v2Tag};
use crate::TaggedFile;
use std::io::{self, Read, Seek, SeekFrom};

const FIELD_BYTES: usize = 4 * 1024;
const TOTAL_BYTES: usize = 16 * 1024;

fn invalid_iff() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid WAVE or AIFF chunk")
}

struct Chunk {
    kind: [u8; 4],
    size: u32,
    content_end: u64,
    padded_end: u64,
}

fn next_chunk(
    reader: &mut (impl Read + Seek),
    parent_end: u64,
    little_endian: bool,
) -> io::Result<Option<Chunk>> {
    let position = reader.stream_position()?;
    let available = parent_end.checked_sub(position).ok_or_else(invalid_iff)?;
    if available == 0 {
        return Ok(None);
    }
    if available < 8 {
        return Err(invalid_iff());
    }
    let mut header = [0; 8];
    reader.read_exact(&mut header)?;
    let size = header[4..].try_into().unwrap();
    let size = if little_endian { u32::from_le_bytes(size) } else { u32::from_be_bytes(size) };
    let content_end = position.checked_add(8 + u64::from(size)).ok_or_else(invalid_iff)?;
    let padded_end = content_end.checked_add(u64::from(size & 1))
        .filter(|end| *end <= parent_end).ok_or_else(invalid_iff)?;
    Ok(Some(Chunk { kind: header[..4].try_into().unwrap(), size, content_end, padded_end }))
}

fn scan_text(
    reader: &mut impl Read,
    size: usize,
    key_len: usize,
    trim_nul: bool,
    budget: &mut usize,
) -> io::Result<Option<String>> {
    if size > FIELD_BYTES || key_len > *budget {
        return Ok(None);
    }
    let mut buffer = [0; FIELD_BYTES];
    reader.read_exact(&mut buffer[..size])?;
    let text = std::str::from_utf8(&buffer[..size]).map_err(|_| invalid_iff())?;
    let text = if trim_nul { text.trim_end_matches('\0') } else { text };
    let cost = key_len + text.len();
    if cost > *budget {
        return Ok(None);
    }
    *budget -= cost;
    Ok(Some(text.to_owned()))
}

fn merge_id3(target: &mut Option<ID3v2Tag>, tag: ID3v2Tag) {
    if let Some(existing) = target {
        for frame in tag {
            drop(existing.insert(frame));
        }
    } else {
        *target = Some(tag);
    }
}

fn read_riff_info(
    reader: &mut (impl Read + Seek),
    list_end: u64,
    budget: &mut usize,
    info: &mut RIFFInfoList,
) -> io::Result<()> {
    while let Some(item) = next_chunk(reader, list_end, true)? {
        if !item.kind.iter().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()) {
            return Err(invalid_iff());
        }
        if let Some(value) = scan_text(reader, item.size as usize, 4, true, budget)? {
            let key = std::str::from_utf8(&item.kind).map_err(|_| invalid_iff())?;
            info.items.push((key.to_owned(), value));
        }
        reader.seek(SeekFrom::Start(item.padded_end))?;
    }
    Ok(())
}

fn read_wave(
    reader: &mut (impl Read + Seek),
    end: u64,
    file_len: u64,
) -> Result<TaggedFile> {
    let mut format = [0; 40];
    let mut format_len = 0;
    let mut sample_count = 0;
    let mut audio_bytes = 0;
    let mut info = RIFFInfoList::default();
    let mut id3 = None;
    let mut budget = TOTAL_BYTES;
    while let Some(chunk) = next_chunk(reader, end, true)? {
        match &chunk.kind {
            b"fmt " if format_len == 0 => {
                format_len = (chunk.size as usize).min(format.len());
                if format_len < 16 {
                    return Err(invalid_iff().into());
                }
                reader.read_exact(&mut format[..format_len])?;
                let channels = u16::from_le_bytes(format[2..4].try_into().unwrap());
                if !(1..=8).contains(&channels) {
                    return Err(invalid_iff().into());
                }
            }
            b"fact" if sample_count == 0 => {
                let mut value = [0; 4];
                if chunk.size < 4 {
                    return Err(invalid_iff().into());
                }
                reader.read_exact(&mut value)?;
                sample_count = u32::from_le_bytes(value);
            }
            b"data" if audio_bytes == 0 => audio_bytes = chunk.size,
            b"LIST" => {
                let mut kind = [0; 4];
                if chunk.size < 4 {
                    return Err(invalid_iff().into());
                }
                reader.read_exact(&mut kind)?;
                if &kind == b"INFO" {
                    read_riff_info(reader, chunk.content_end, &mut budget, &mut info)?;
                }
            }
            b"ID3 " | b"id3 " => {
                merge_id3(&mut id3, read_id3v2_for_scan(reader, chunk.content_end, &mut budget)?);
            }
            _ => {}
        }
        reader.seek(SeekFrom::Start(chunk.padded_end))?;
    }
    if format_len < 16 || audio_bytes == 0 {
        return Err(invalid_iff().into());
    }
    let properties = super::wav::properties::read_properties(
        &mut &format[..format_len], sample_count, audio_bytes, file_len,
    )?;
    Ok(WavFile { properties, riff_info_tag: (!info.items.is_empty()).then_some(info), id3v2_tag: id3 }.into())
}

fn read_aiff_comments(
    reader: &mut (impl Read + Seek),
    end: u64,
    budget: &mut usize,
    comments: &mut Vec<Comment>,
) -> io::Result<()> {
    let mut count = [0; 2];
    if end.checked_sub(reader.stream_position()?).is_none_or(|available| available < 2) {
        return Err(invalid_iff());
    }
    reader.read_exact(&mut count)?;
    for _ in 0..u16::from_be_bytes(count) {
        let position = reader.stream_position()?;
        if end.checked_sub(position).is_none_or(|available| available < 8) {
            return Err(invalid_iff());
        }
        let mut header = [0; 8];
        reader.read_exact(&mut header)?;
        let length = u16::from_be_bytes(header[6..].try_into().unwrap());
        let next = position.checked_add(8 + u64::from(length) + u64::from(length & 1))
            .filter(|next| *next <= end).ok_or_else(invalid_iff)?;
        if let Some(text) = scan_text(reader, usize::from(length), 4, false, budget)? {
            comments.push(Comment {
                timestamp: u32::from_be_bytes(header[..4].try_into().unwrap()),
                marker_id: u16::from_be_bytes(header[4..6].try_into().unwrap()),
                text,
            });
        }
        reader.seek(SeekFrom::Start(next))?;
    }
    Ok(())
}

fn read_aiff(
    reader: &mut (impl Read + Seek),
    end: u64,
    file_len: u64,
) -> Result<TaggedFile> {
    let mut common = None;
    let mut audio_bytes = 0;
    let mut text = AIFFTextChunks::default();
    let mut id3 = None;
    let mut budget = TOTAL_BYTES;
    while let Some(chunk) = next_chunk(reader, end, false)? {
        match &chunk.kind {
            b"COMM" if common.is_none() => {
                if chunk.size < 18 {
                    return Err(invalid_iff().into());
                }
                let mut bytes = [0; 18];
                reader.read_exact(&mut bytes)?;
                let channels = u16::from_be_bytes(bytes[..2].try_into().unwrap());
                if !(1..=8).contains(&channels) {
                    return Err(invalid_iff().into());
                }
                common = Some(bytes);
            }
            b"SSND" => audio_bytes = chunk.size,
            b"ID3 " | b"id3 " => {
                merge_id3(&mut id3, read_id3v2_for_scan(reader, chunk.content_end, &mut budget)?);
            }
            b"COMT" if text.comments.is_none() => {
                let mut comments = Vec::new();
                read_aiff_comments(reader, chunk.content_end, &mut budget, &mut comments)?;
                text.comments = (!comments.is_empty()).then_some(comments);
            }
            b"NAME" if text.name.is_none() => {
                text.name = scan_text(reader, chunk.size as usize, 4, false, &mut budget)?;
            }
            b"AUTH" if text.author.is_none() => {
                text.author = scan_text(reader, chunk.size as usize, 4, false, &mut budget)?;
            }
            b"(c) " if text.copyright.is_none() => {
                text.copyright = scan_text(reader, chunk.size as usize, 4, false, &mut budget)?;
            }
            b"ANNO" => {
                if let Some(annotation) = scan_text(reader, chunk.size as usize, 4, false, &mut budget)? {
                    text.annotations.get_or_insert_with(Vec::new).push(annotation);
                }
            }
            _ => {}
        }
        reader.seek(SeekFrom::Start(chunk.padded_end))?;
    }
    let bytes = common.ok_or_else(invalid_iff)?;
    if audio_bytes == 0 {
        return Err(invalid_iff().into());
    }
    let properties = super::aiff::properties::read_properties(
        &mut &bytes[..], audio_bytes, file_len,
    )?;
    let text_chunks_tag = (text != AIFFTextChunks::default()).then_some(text);
    Ok(AiffFile { properties, text_chunks_tag, id3v2_tag: id3 }.into())
}

/// Read supported IFF metadata from the existing scanner handle.
pub fn read_iff_for_scan<R: Read + Seek>(reader: &mut R) -> Result<Option<TaggedFile>> {
    let start = reader.stream_position()?;
    let file_len = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(start))?;
    let mut header = [0; 12];
    reader.read_exact(&mut header)?;
    let little_endian = match (&header[..4], &header[8..]) {
        (b"RIFF", b"WAVE") => true,
        (b"FORM", b"AIFF" | b"AIFC") => false,
        _ => return Ok(None),
    };
    let length = header[4..8].try_into().unwrap();
    let length = if little_endian { u32::from_le_bytes(length) } else { u32::from_be_bytes(length) };
    let end = start.checked_add(8 + u64::from(length))
        .filter(|end| *end <= file_len && length >= 4).ok_or_else(invalid_iff)?;
    if little_endian {
        read_wave(reader, end, file_len).map(Some)
    } else {
        read_aiff(reader, end, file_len).map(Some)
    }
}
