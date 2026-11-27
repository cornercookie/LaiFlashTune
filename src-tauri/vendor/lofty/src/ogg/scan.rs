//! Bounded Vorbis and Opus comments for library scans.

use super::constants::{OPUSHEAD, OPUSTAGS, VORBIS_COMMENT_HEAD, VORBIS_IDENT_HEAD};
use super::{opus, vorbis, OpusFile, VorbisComments, VorbisFile};
use crate::error::Result;
use crate::TaggedFile;
use ogg_pager::Page;
use std::io::{self, Read, Seek, SeekFrom};

const FIELD_BYTES: usize = 4 * 1024;
const TAG_BYTES: usize = 16 * 1024;

fn invalid_ogg() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid Ogg comment packet")
}

struct PacketReader<'a, R> {
    file: &'a mut R,
    end: u64,
    serial: Option<u32>,
    next_page_number: u32,
    page_start: u64,
    page_end: u64,
    laces: [u8; 255],
    lace_count: usize,
    next_lace: usize,
    segment_remaining: usize,
    packet_started: bool,
    packet_ended: bool,
}

impl<'a, R: Read + Seek> PacketReader<'a, R> {
    fn new(file: &'a mut R, end: u64) -> Self {
        Self {
            file, end, serial: None, next_page_number: 0, page_start: 0, page_end: 0,
            laces: [0; 255], lace_count: 0, next_lace: 0, segment_remaining: 0,
            packet_started: false, packet_ended: false,
        }
    }

    fn read_page(&mut self) -> io::Result<()> {
        loop {
            let page_start = self.file.stream_position()?;
            let mut header = [0; 27];
            self.file.read_exact(&mut header)?;
            if &header[..4] != b"OggS" || header[4] != 0 {
                return Err(invalid_ogg());
            }
            let lace_count = usize::from(header[26]);
            self.file.read_exact(&mut self.laces[..lace_count])?;
            let body_len: u64 = self.laces[..lace_count].iter().map(|n| u64::from(*n)).sum();
            let page_end = self.file.stream_position()?.checked_add(body_len)
                .filter(|end| *end <= self.end).ok_or_else(invalid_ogg)?;
            let serial = u32::from_le_bytes(header[14..18].try_into().unwrap());
            if self.serial.is_some_and(|expected| serial != expected) {
                self.file.seek(SeekFrom::Start(page_end))?;
                continue;
            }
            let page_number = u32::from_le_bytes(header[18..22].try_into().unwrap());
            if (header[5] & 0x01 != 0) != self.packet_started
                || (self.serial.is_some() && page_number != self.next_page_number)
            {
                return Err(invalid_ogg());
            }
            self.serial = Some(serial);
            self.next_page_number = page_number.wrapping_add(1);
            self.lace_count = lace_count;
            self.next_lace = 0;
            self.page_start = page_start;
            self.page_end = page_end;
            return Ok(());
        }
    }

    fn has_segment(&mut self) -> io::Result<bool> {
        while self.segment_remaining == 0 && !self.packet_ended {
            if self.next_lace == self.lace_count {
                self.read_page()?;
                if self.lace_count == 0 {
                    continue;
                }
            }
            let length = self.laces[self.next_lace];
            self.next_lace += 1;
            self.segment_remaining = usize::from(length);
            self.packet_started = true;
            self.packet_ended = length < 255;
        }
        Ok(self.segment_remaining > 0)
    }

    fn skip(&mut self, mut bytes: usize) -> io::Result<()> {
        while bytes > 0 {
            if !self.has_segment()? {
                return Err(invalid_ogg());
            }
            let skipped = bytes.min(self.segment_remaining);
            self.file.seek(SeekFrom::Current(skipped as i64))?;
            self.segment_remaining -= skipped;
            bytes -= skipped;
        }
        Ok(())
    }

    fn next_packet(&mut self) -> io::Result<()> {
        while self.has_segment()? {
            self.skip(self.segment_remaining)?;
        }
        self.packet_started = false;
        self.packet_ended = false;
        Ok(())
    }

    fn read_length(&mut self) -> io::Result<usize> {
        let mut length = [0; 4];
        self.read_exact(&mut length)?;
        Ok(u32::from_le_bytes(length) as usize)
    }
}

impl<R: Read + Seek> Read for PacketReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || !self.has_segment()? {
            return Ok(0);
        }
        let length = buffer.len().min(self.segment_remaining);
        let count = self.file.read(&mut buffer[..length])?;
        if count == 0 {
            return Err(invalid_ogg());
        }
        self.segment_remaining -= count;
        Ok(count)
    }
}

fn read_scan_comments<R: Read + Seek>(reader: &mut PacketReader<'_, R>) -> io::Result<VorbisComments> {
    let vendor_len = reader.read_length()?;
    reader.skip(vendor_len)?;
    let count = reader.read_length()?;
    let mut comments = VorbisComments::default();
    let mut remaining_budget = TAG_BYTES;
    let mut key = [0u8; FIELD_BYTES];
    let mut value = [0u8; FIELD_BYTES];

    for _ in 0..count {
        let mut remaining = reader.read_length()?;
        if remaining_budget == 0 {
            reader.skip(remaining)?;
            continue;
        }
        let mut key_len = 0;
        let mut delimiter = false;
        while remaining > 0 && key_len <= key.len() {
            let mut byte = [0];
            reader.read_exact(&mut byte)?;
            remaining -= 1;
            if byte[0] == b'=' {
                delimiter = true;
                break;
            }
            if key_len == key.len() || !(0x20..=0x7d).contains(&byte[0]) {
                break;
            }
            key[key_len] = byte[0];
            key_len += 1;
        }
        let key_bytes = &key[..key_len];
        if !delimiter
            || key_len == 0
            || remaining > FIELD_BYTES
            || key_len + remaining > remaining_budget
            || key_bytes.eq_ignore_ascii_case(b"METADATA_BLOCK_PICTURE")
            || key_bytes.eq_ignore_ascii_case(b"COVERART")
        {
            reader.skip(remaining)?;
            continue;
        }
        reader.read_exact(&mut value[..remaining])?;
        let name = std::str::from_utf8(key_bytes).map_err(|_| invalid_ogg())?;
        let text = std::str::from_utf8(&value[..remaining]).map_err(|_| invalid_ogg())?;
        remaining_budget -= key_len + text.len();
        comments.insert(name.to_owned(), text.to_owned(), false);
    }
    Ok(comments)
}

/// Reads Vorbis or Opus metadata without collecting an Ogg packet in memory.
/// Unknown Ogg codec identifiers are left to the caller's existing routing.
pub fn read_ogg_for_scan<R: Read + Seek>(file: &mut R) -> Result<Option<TaggedFile>> {
    let start = file.stream_position()?;
    let end = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(start))?;
    let first_page = Page::read(file, false)?;
    let head = first_page.content();
    let vorbis = head.starts_with(VORBIS_IDENT_HEAD);
    let expected_comment = if vorbis {
        VORBIS_COMMENT_HEAD
    } else if head.starts_with(OPUSHEAD) {
        OPUSTAGS
    } else {
        return Ok(None);
    };

    file.seek(SeekFrom::Start(start))?;
    let (comments, audio_start) = {
        let mut packets = PacketReader::new(file, end);
        packets.next_packet()?;
        let mut signature = [0; 8];
        packets.read_exact(&mut signature[..expected_comment.len()])?;
        if &signature[..expected_comment.len()] != expected_comment {
            return Err(invalid_ogg().into());
        }
        let comments = read_scan_comments(&mut packets)?;
        packets.next_packet()?;
        let audio_start = if packets.page_end == end { packets.page_start } else { packets.page_end };
        (comments, audio_start)
    };

    file.seek(SeekFrom::Start(audio_start))?;
    let tagged = if vorbis {
        VorbisFile {
            properties: vorbis::properties::read_properties(file, &first_page)?,
            vorbis_comments_tag: comments,
        }.into()
    } else {
        OpusFile {
            properties: opus::properties::read_properties(file, &first_page)?,
            vorbis_comments_tag: comments,
        }.into()
    };
    Ok(Some(tagged))
}
