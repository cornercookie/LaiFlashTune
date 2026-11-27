//! Bounded MP4 library metadata and audio-property scan.

use super::atom_info::{AtomIdent, AtomInfo};
use super::{Atom, AtomData, Ilst};
use super::properties::{alac_properties, flac_properties, mp4a_properties, Mp4Properties};
use super::read::{meta_is_full, AtomReader};
use super::Mp4File;
use crate::error::Result;
use crate::id3::v1::constants::GENRES;
use crate::util::text::utf16_decode;
use crate::TaggedFile;
use std::io::{self, Read, Seek, SeekFrom};
use std::time::Duration;

const FIELD_BYTES: usize = 4 * 1024;
const TAG_BYTES: usize = 16 * 1024;

fn invalid_mp4() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid MP4 atom in library scan")
}

#[derive(Clone, Copy)]
struct AtomRange {
    kind: [u8; 4],
    body: u64,
    end: u64,
}

impl AtomRange {
    fn len(self) -> u64 {
        self.end - self.body
    }
}

fn next_atom(reader: &mut (impl Read + Seek), parent_end: u64) -> io::Result<Option<AtomRange>> {
    let start = reader.stream_position()?;
    let available = parent_end.checked_sub(start).ok_or_else(invalid_mp4)?;
    if available == 0 {
        return Ok(None);
    }
    if available < 8 {
        return Err(invalid_mp4());
    }
    let mut header = [0; 8];
    reader.read_exact(&mut header)?;
    let small = u32::from_be_bytes(header[..4].try_into().unwrap());
    let (size, header_len) = match small {
        0 => (available, 8),
        1 if available >= 16 => {
            let mut length = [0; 8];
            reader.read_exact(&mut length)?;
            (u64::from_be_bytes(length), 16)
        }
        1 => return Err(invalid_mp4()),
        size => (u64::from(size), 8),
    };
    if size < header_len || size > available {
        return Err(invalid_mp4());
    }
    Ok(Some(AtomRange {
        kind: header[4..].try_into().unwrap(),
        body: start + header_len,
        end: start + size,
    }))
}

fn child(reader: &mut (impl Read + Seek), parent: AtomRange, kind: &[u8; 4]) -> io::Result<Option<AtomRange>> {
    reader.seek(SeekFrom::Start(parent.body))?;
    while let Some(atom) = next_atom(reader, parent.end)? {
        if &atom.kind == kind {
            return Ok(Some(atom));
        }
        reader.seek(SeekFrom::Start(atom.end))?;
    }
    Ok(None)
}

fn read_atom_text(reader: &mut (impl Read + Seek), atom: AtomRange) -> io::Result<Option<String>> {
    let size = atom.len().checked_sub(4).ok_or_else(invalid_mp4)?;
    if size > FIELD_BYTES as u64 {
        return Ok(None);
    }
    reader.seek(SeekFrom::Start(atom.body + 4))?;
    let mut input = [0; FIELD_BYTES];
    let input = &mut input[..size as usize];
    reader.read_exact(input)?;
    let value = std::str::from_utf8(input).map_err(|_| invalid_mp4())?;
    Ok(Some(value.to_owned()))
}

fn identify_item(
    reader: &mut (impl Read + Seek),
    atom: AtomRange,
) -> io::Result<Option<(AtomIdent, usize)>> {
    if &atom.kind == b"----" {
        let mean = child(reader, atom, b"mean")?.ok_or_else(invalid_mp4)?;
        let name = child(reader, atom, b"name")?.ok_or_else(invalid_mp4)?;
        let (Some(mean), Some(name)) = (read_atom_text(reader, mean)?, read_atom_text(reader, name)?) else {
            return Ok(None);
        };
        let key_size = "----::".len() + mean.len() + name.len();
        if key_size > FIELD_BYTES {
            return Ok(None);
        }
        return Ok(Some((AtomIdent::Freeform { mean, name }, key_size)));
    }
    let key = if &atom.kind == b"gnre" { *b"\xa9gen" } else { atom.kind };
    let key_size = key.iter().map(|&byte| char::from(byte).len_utf8()).sum();
    Ok(Some((AtomIdent::Fourcc(key), key_size)))
}

fn decode_item(
    reader: &mut (impl Read + Seek),
    data: AtomRange,
    key: &[u8; 4],
) -> Result<Option<(AtomData, usize)>> {
    let size = data.len().checked_sub(8).ok_or_else(invalid_mp4)?;
    if size == 0 || size > FIELD_BYTES as u64 {
        return Ok(None);
    }
    let mut header = [0; 8];
    reader.read_exact(&mut header)?;
    let flags = u32::from_be_bytes([0, header[1], header[2], header[3]]);
    let number_pair = flags == 0 && matches!(key, b"trkn" | b"disk") && (6..=8).contains(&size);
    let genre = matches!(flags, 0 | 21) && key == b"gnre" && size == 2;
    if !matches!(flags, 1 | 2) && !number_pair && !genre {
        return Ok(None);
    }
    let mut input = [0; FIELD_BYTES];
    let input = &mut input[..size as usize];
    reader.read_exact(input)?;
    if number_pair {
        let current = u16::from_be_bytes([input[2], input[3]]);
        let total = u16::from_be_bytes([input[4], input[5]]);
        let cost = current.to_string().len() + total.to_string().len();
        return Ok(Some((AtomData::Unknown { code: 0, data: input.to_vec() }, cost)));
    }
    let value = if genre {
        let index = usize::from(input[1]);
        let Some(value) = index.checked_sub(1).and_then(|index| GENRES.get(index)) else {
            return Ok(None);
        };
        AtomData::UTF8((*value).to_owned())
    } else if flags == 1 {
        let value = std::str::from_utf8(input).map_err(|_| invalid_mp4())?;
        AtomData::UTF8(value.to_owned())
    } else {
        AtomData::UTF16(utf16_decode(input, u16::from_be_bytes)?)
    };
    let cost = match &value {
        AtomData::UTF8(value) | AtomData::UTF16(value) if value.len() <= FIELD_BYTES => value.len(),
        _ => return Ok(None),
    };
    Ok(Some((value, cost)))
}

fn read_ilst(
    reader: &mut (impl Read + Seek),
    list: AtomRange,
    budget: &mut usize,
    tag: &mut Ilst,
) -> Result<()> {
    reader.seek(SeekFrom::Start(list.body))?;
    while let Some(item) = next_atom(reader, list.end)? {
        if !matches!(&item.kind, b"covr" | b"free" | b"skip") && *budget > 0 {
            if let Some((key, key_size)) = identify_item(reader, item)? {
                reader.seek(SeekFrom::Start(item.body))?;
                let mut accepted = Vec::new();
                while let Some(data) = next_atom(reader, item.end)? {
                    if &data.kind == b"data" && key_size <= *budget {
                        if let Some((value, cost)) = decode_item(reader, data, &item.kind)? {
                            if key_size + cost <= *budget {
                                *budget -= key_size + cost;
                                accepted.push(value);
                            }
                        }
                    }
                    reader.seek(SeekFrom::Start(data.end))?;
                }
                if let Some(atom) = Atom::from_collection(key, accepted) {
                    tag.atoms.push(atom);
                }
            }
        }
        reader.seek(SeekFrom::Start(item.end))?;
    }
    Ok(())
}

fn read_metadata(
    reader: &mut (impl Read + Seek),
    mut meta: AtomRange,
    budget: &mut usize,
    tag: &mut Ilst,
) -> Result<()> {
    if meta.len() < 8 {
        return if meta.len() <= 4 { Ok(()) } else { Err(invalid_mp4().into()) };
    }
    reader.seek(SeekFrom::Start(meta.body))?;
    if meta_is_full(reader)? {
        meta.body += 4;
    }
    if let Some(list) = child(reader, meta, b"ilst")? {
        read_ilst(reader, list, budget, tag)?;
    }
    Ok(())
}

struct Region<'a, R> {
    reader: &'a mut R,
    base: u64,
    length: u64,
    position: u64,
}

impl<R: Read + Seek> Read for Region<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let limit = (self.length - self.position).min(output.len() as u64) as usize;
        let count = self.reader.read(&mut output[..limit])?;
        self.position += count as u64;
        Ok(count)
    }
}

impl<R: Read + Seek> Seek for Region<'_, R> {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        let position = match target {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.length) + i128::from(delta),
        };
        let position = u64::try_from(position).ok()
            .filter(|position| *position <= self.length).ok_or_else(invalid_mp4)?;
        self.reader.seek(SeekFrom::Start(self.base + position))?;
        self.position = position;
        Ok(position)
    }
}

fn is_audio_track(reader: &mut (impl Read + Seek), media: AtomRange) -> io::Result<bool> {
    let Some(handler) = child(reader, media, b"hdlr")? else { return Ok(false) };
    if handler.len() < 12 {
        return Err(invalid_mp4());
    }
    reader.seek(SeekFrom::Start(handler.body + 8))?;
    let mut kind = [0; 4];
    reader.read_exact(&mut kind)?;
    Ok(&kind == b"soun")
}

fn read_audio_properties(
    reader: &mut (impl Read + Seek),
    media: AtomRange,
    file_len: u64,
    audio_bytes: u64,
) -> Result<Mp4Properties> {
    let timing = child(reader, media, b"mdhd")?.ok_or_else(invalid_mp4)?;
    let mut header = [0; 32];
    let read_len = timing.len().min(header.len() as u64) as usize;
    reader.seek(SeekFrom::Start(timing.body))?;
    reader.read_exact(&mut header[..read_len])?;
    let (scale, frames) = match header[0] {
        0 if read_len >= 20 => (
            u32::from_be_bytes(header[12..16].try_into().unwrap()),
            u64::from(u32::from_be_bytes(header[16..20].try_into().unwrap())),
        ),
        1 if read_len >= 32 => (
            u32::from_be_bytes(header[20..24].try_into().unwrap()),
            u64::from_be_bytes(header[24..32].try_into().unwrap()),
        ),
        _ => return Err(invalid_mp4().into()),
    };
    if scale == 0 {
        return Err(invalid_mp4().into());
    }
    let duration = u64::try_from(u128::from(frames) * 1000 / u128::from(scale))
        .map_err(|_| invalid_mp4())?;
    let mut props = Mp4Properties { duration: Duration::from_millis(duration), ..Mp4Properties::default() };

    if let Some(minf) = child(reader, media, b"minf")? {
        if let Some(stbl) = child(reader, minf, b"stbl")? {
            if let Some(stsd) = child(reader, stbl, b"stsd")? {
                let mut region = Region { reader, base: stsd.body, length: stsd.len(), position: 0 };
                region.seek(SeekFrom::Start(0))?;
                let mut stsd_reader = AtomReader::new_without_freeform(region)?;
                stsd_reader.seek(SeekFrom::Start(8))?;
                let sample = AtomInfo::read_impl(&mut stsd_reader, stsd.len() - 8, false)?;
                if let AtomIdent::Fourcc(kind) = sample.ident {
                    match &kind {
                        b"mp4a" => mp4a_properties(&mut stsd_reader, &mut props)?,
                        b"alac" => alac_properties(&mut stsd_reader, &mut props)?,
                        b"fLaC" => flac_properties(&mut stsd_reader, &mut props)?,
                        _ => {}
                    }
                }
            }
        }
    }
    if duration > 0 {
        let bitrate = |bytes: u64| {
            let rate = u128::from(bytes) * 8 / u128::from(duration);
            u32::try_from(rate).unwrap_or(u32::MAX)
        };
        props.overall_bitrate = bitrate(file_len);
        if props.audio_bitrate == 0 {
            props.audio_bitrate = bitrate(audio_bytes);
        }
    }
    Ok(props)
}

/// Reads bounded MP4 library metadata from a seekable, already-opened file.
pub fn read_mp4_for_scan<R: Read + Seek>(reader: &mut R) -> Result<TaggedFile> {
    let start = reader.stream_position()?;
    let end = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(start))?;
    let ftyp = next_atom(reader, end)?.filter(|atom| &atom.kind == b"ftyp" && atom.len() >= 4)
        .ok_or_else(invalid_mp4)?;
    let mut brand = [0; 4];
    reader.read_exact(&mut brand)?;
    let ftyp_name = std::str::from_utf8(&brand).map_err(|_| invalid_mp4())?.to_owned();
    reader.seek(SeekFrom::Start(ftyp.end))?;

    let mut budget = TAG_BYTES;
    let mut tag = Ilst::default();
    let mut media = None;
    let mut audio_bytes = 0u64;
    while let Some(atom) = next_atom(reader, end)? {
        match &atom.kind {
            b"meta" => read_metadata(reader, atom, &mut budget, &mut tag)?,
            b"moov" => {
                reader.seek(SeekFrom::Start(atom.body))?;
                while let Some(movie) = next_atom(reader, atom.end)? {
                    match &movie.kind {
                        b"meta" => read_metadata(reader, movie, &mut budget, &mut tag)?,
                        b"udta" => {
                            if let Some(meta) = child(reader, movie, b"meta")? {
                                read_metadata(reader, meta, &mut budget, &mut tag)?;
                            }
                        }
                        b"trak" if media.is_none() => {
                            if let Some(candidate) = child(reader, movie, b"mdia")? {
                                if is_audio_track(reader, candidate)? {
                                    media = Some(candidate);
                                }
                            }
                        }
                        _ => {}
                    }
                    reader.seek(SeekFrom::Start(movie.end))?;
                }
            }
            b"mdat" => audio_bytes = audio_bytes.checked_add(atom.len()).ok_or_else(invalid_mp4)?,
            _ => {}
        }
        reader.seek(SeekFrom::Start(atom.end))?;
    }
    let properties = read_audio_properties(reader, media.ok_or_else(invalid_mp4)?, end, audio_bytes)?;
    let ilst_tag = (!tag.atoms.is_empty()).then_some(tag);
    Ok(Mp4File { ftyp: ftyp_name, ilst_tag, properties }.into())
}
