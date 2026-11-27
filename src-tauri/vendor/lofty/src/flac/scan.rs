//! Defensive scan-only reader for native FLAC files.
//!
//! [`read_native_flac_for_scan`] is a restricted FLAC reader intended for
//! metadata scanning. Only native `fLaC` streams are accepted, the first
//! metadata block must be a standard 34-byte `STREAMINFO` block, every
//! metadata block header is bounds-checked against the actual file length
//! using checked arithmetic, and picture or otherwise unparsed blocks are
//! seeked over without decoding their contents. Vorbis comments are parsed
//! with a fixed key buffer and a per-file key/value budget, so no allocation
//! is driven by untrusted declarations.

use crate::error::Result;
use crate::file::{FileType, TaggedFile};
use crate::macros::decode_err;
use crate::tag::{Tag, TagType};
use crate::tag::item::{ItemKey, ItemValue, TagItem};
use super::properties;

use std::io::{Read, Seek, SeekFrom};

// Fixed body size of the standard FLAC `STREAMINFO` metadata block.
const STREAMINFO_LEN: u64 = 34;

// Longest accepted Vorbis comment key, in bytes.
const MAX_KEY_LEN: usize = 4096;

// Longest accepted Vorbis comment value, in bytes.
const MAX_VALUE_LEN: usize = 4096;

// Combined accepted key+value budget, in bytes, consumed per file.
const TOTAL_TEXT_BUDGET: usize = 16384;

/// Read a native FLAC file defensively for scanning purposes.
///
/// Optional ID3v2 prefixes and the native `fLaC` marker are checked on the same handle.
/// The first type 0 `STREAMINFO` block is also validated, where the
/// `STREAMINFO` body must be exactly 34 bytes. Every metadata block header is
/// read as a file position, a `u24` length, and a checked end offset compared
/// against the actual file length. Blocks of type 1, 3, 6 and every other
/// unparsed block are seeked to their validated end offset, so picture
/// content is skipped without decoding. Type 4 `VORBIS_COMMENT` blocks are
/// parsed with fixed, checked limits: vendor and item payloads may not exceed
/// the remaining block bytes, keys are read into a reusable 4 KiB buffer up to
/// the first `=`, values are capped at 4 KiB, and accepted keys plus values
/// share a 16 KiB per-file budget.
///
/// Audio properties are computed with the existing
/// [`crate::flac::properties::read_properties`] helper, and the result keeps
/// the normal `FlacFile` to `TaggedFile` field mapping. No embedded picture
/// thumbnail is produced and picture-only files yield no tag.
///
/// # Errors
///
/// Returns an I/O error when the handle cannot be seeked or read, and a
/// format error when the stream is not native FLAC, lacks a standard
/// `STREAMINFO` block, or contains bounds-violating metadata blocks.
pub fn read_native_flac_for_scan<R: Read + Seek>(data: &mut R) -> Result<TaggedFile> {
	let file_len = data.seek(SeekFrom::End(0))?;
	data.seek(SeekFrom::Start(0))?;
	let prefix_remaining = TOTAL_TEXT_BUDGET;
	#[cfg(feature = "id3v2")]
	let (id3_prefix, prefix_remaining) = {
		let mut remaining = prefix_remaining;
		let prefix = crate::id3::v2::scan::read_id3v2_prefixes(data, file_len, &mut remaining)?;
		(prefix, remaining)
	};

	let mut marker = [0u8; 4];
	data.read_exact(&mut marker)?;
	if &marker != b"fLaC" {
		decode_err!(@BAIL FLAC, "File missing \"fLaC\" stream marker");
	}

	// The first metadata block must be a standard 34-byte STREAMINFO block.
	let (mut last_block, mut audio_start, mut stream_info) = {
		let (block_last, ty, len, block_end) = read_block_header(data, file_len)?;

		if ty != 0 {
			decode_err!(@BAIL FLAC, "File missing mandatory STREAMINFO block");
		}

		if len != STREAMINFO_LEN {
			decode_err!(@BAIL FLAC, "File has a non-standard STREAMINFO block size");
		}

		let mut stream_info = [0u8; STREAMINFO_LEN as usize];
		data.read_exact(&mut stream_info)?;

		(block_last, block_end, stream_info)
	};

	let mut tag = Tag::new(TagType::VorbisComments);
	#[cfg(feature = "vorbis_comments")]
	let mut used = TOTAL_TEXT_BUDGET - prefix_remaining;
	#[cfg(feature = "vorbis_comments")]
	let mut key_buf = [0u8; MAX_KEY_LEN];

	while !last_block {
		let (block_last, ty, len, block_end) = read_block_header(data, file_len)?;
		last_block = block_last;

		if len == 0 && ty != 1 && ty != 3 {
			decode_err!(@BAIL FLAC, "Encountered a zero-sized metadata block");
		}

		#[cfg(feature = "vorbis_comments")]
		if ty == 4 {
			read_vorbis_comments_block(data, block_end, &mut used, &mut tag, &mut key_buf)?;
		}

		data.seek(SeekFrom::Start(block_end))?;
		audio_start = block_end;
	}

	let stream_length = file_len - audio_start;
	let mut stream_info_reader = &stream_info[..];
	let file_properties =
		properties::read_properties(&mut stream_info_reader, stream_length, file_len)?;

	let mut tags = Vec::new();
	if tag.item_count() > 0 {
		tags.push(tag);
	}
	#[cfg(feature = "id3v2")]
	if let Some(prefix) = id3_prefix {
		tags.push(prefix.into());
	}

	Ok(TaggedFile::new(FileType::FLAC, file_properties, tags))
}

/// Read one metadata block header from `data`.
///
/// Returns the last-block flag, block type, declared length, and the checked
/// absolute end offset of the block, validated against `file_len`.
fn read_block_header<R: Read + Seek>(
	data: &mut R,
	file_len: u64,
) -> Result<(bool, u8, u64, u64)> {
	let start = data.stream_position()?;

	let mut header = [0u8; 4];
	data.read_exact(&mut header)?;

	let last = (header[0] & 0x80) != 0;
	let ty = header[0] & 0x7F;
	let len = u32::from_be_bytes([0, header[1], header[2], header[3]]) as u64;

	let end = start
		.checked_add(4)
		.and_then(|pos| pos.checked_add(len))
		.ok_or_else(|| decode_err!(FLAC, "Metadata block length overflow"))?;

	if end > file_len {
		decode_err!(@BAIL FLAC, "Metadata block extends beyond end of file");
	}

	Ok((last, ty, len, end))
}

#[cfg(feature = "vorbis_comments")]
fn read_vorbis_comments_block<R: Read + Seek>(
	data: &mut R,
	block_end: u64,
	used: &mut usize,
	tag: &mut Tag,
	key_buf: &mut [u8; MAX_KEY_LEN],
) -> Result<()> {
	let mut remaining = block_end - data.stream_position()?;

	// Vendor string length field, then its payload. The vendor is skipped
	// without allocation and without UTF-8 validation; neither counts toward
	// the text budget.
	if remaining < 4 {
		decode_err!(@BAIL FLAC, "Vorbis comments block is truncated");
	}

	let mut len_bytes = [0u8; 4];
	data.read_exact(&mut len_bytes)?;
	remaining -= 4;

	let vendor_len = u32::from_le_bytes(len_bytes) as u64;
	if vendor_len > remaining {
		decode_err!(@BAIL FLAC, "Vendor length exceeds Vorbis comments block");
	}
	remaining -= vendor_len;
	data.seek(SeekFrom::Current(vendor_len as i64))?;

	// Item count field. The declared count is never used to reserve storage;
	// each item is bounds-checked against the remaining block bytes instead.
	if remaining < 4 {
		decode_err!(@BAIL FLAC, "Vorbis comments block is truncated");
	}
	data.read_exact(&mut len_bytes)?;
	remaining -= 4;
	let count = u32::from_le_bytes(len_bytes);

	for _ in 0..count {
		if remaining < 4 {
			decode_err!(@BAIL FLAC, "Vorbis comments item length field is truncated");
		}
		data.read_exact(&mut len_bytes)?;
		remaining -= 4;

		let item_len = u32::from_le_bytes(len_bytes) as u64;
		if item_len > remaining {
			decode_err!(@BAIL FLAC, "Vorbis comments item length exceeds block");
		}

		let item_start = data.stream_position()?;
		let item_end = item_start
			.checked_add(item_len)
			.ok_or_else(|| decode_err!(FLAC, "Vorbis comments item offset overflow"))?;

		read_vorbis_comment_item(data, item_end, used, tag, key_buf)?;

		data.seek(SeekFrom::Start(item_end))?;
		remaining -= item_len;
	}

	Ok(())
}

#[cfg(feature = "vorbis_comments")]
fn read_vorbis_comment_item<R: Read + Seek>(
	data: &mut R,
	item_end: u64,
	used: &mut usize,
	tag: &mut Tag,
	key_buf: &mut [u8; MAX_KEY_LEN],
) -> Result<()> {
	let mut key_len = 0usize;
	let mut eq_found = false;
	let mut key_invalid = false;

	// Scan the key for the first `=` using a reusable 4 KiB buffer, reading
	// only up to the delimiter. Every key byte must fall within 0x20..=0x7D
	// and must not be `=`.
	while key_len < MAX_KEY_LEN && data.stream_position()? < item_end {
		let pos = data.stream_position()?;
		let remaining_item = item_end - pos;
		let limit = (MAX_KEY_LEN - key_len) as u64;
		let to_read = remaining_item.min(limit) as usize;

		data.read_exact(&mut key_buf[key_len..key_len + to_read])?;

		let mut found_at: Option<usize> = None;
		for (i, &byte) in key_buf[key_len..key_len + to_read].iter().enumerate() {
			if byte == b'=' {
				found_at = Some(i);
				break;
			}
			if byte < 0x20 || byte > 0x7D {
				key_invalid = true;
				break;
			}
		}

		if let Some(i) = found_at {
			eq_found = true;
			key_len += i;
			data.seek(SeekFrom::Start(pos + (i + 1) as u64))?;
			break;
		}

		if key_invalid {
			break;
		}

		key_len += to_read;
	}

	// The 4,097th byte is only inspected when the item still has bytes, to
	// tell whether the 4,096-byte key is terminated by `=`.
	if !eq_found && !key_invalid && key_len == MAX_KEY_LEN && data.stream_position()? < item_end {
		let mut extra = [0u8; 1];
		data.read_exact(&mut extra)?;

		eq_found = extra[0] == b'=';
		if !eq_found {
			key_invalid = true;
		}
	}

	// An empty key, an invalid key, or a key without a delimiter means the
	// whole item is skipped: position to its validated end.
	if !eq_found || key_invalid || key_len == 0 {
		data.seek(SeekFrom::Start(item_end))?;
		return Ok(());
	}

	let value_start = data.stream_position()?;
	let value_len = item_end - value_start;

	// Picture-related keys are always recognised case-insensitively and
	// skipped; their values are never read and they never consume budget.
	if is_picture_key(&key_buf[..key_len]) {
		data.seek(SeekFrom::Start(item_end))?;
		return Ok(());
	}

	if value_len > MAX_VALUE_LEN as u64 {
		data.seek(SeekFrom::Start(item_end))?;
		return Ok(());
	}

	// Accepted keys and values share a per-file budget; over-budget items are
	// skipped without reading or validating their values.
	if *used + key_len + value_len as usize > TOTAL_TEXT_BUDGET {
		data.seek(SeekFrom::Start(item_end))?;
		return Ok(());
	}

	let mut value_bytes = vec![0u8; value_len as usize];
	data.read_exact(&mut value_bytes)?;
	let value = String::from_utf8(value_bytes)?;

	let key_str = String::from_utf8(key_buf[..key_len].to_vec())?;
	let item_key = ItemKey::from_key(TagType::VorbisComments, &key_str);
	tag.push_item_unchecked(TagItem::new(item_key, ItemValue::Text(value)));

	*used += key_len + value_len as usize;
	Ok(())
}

/// Returns true when `key` is `METADATA_BLOCK_PICTURE` or `COVERART`,
/// compared as ASCII without regard to case.
#[cfg(feature = "vorbis_comments")]
fn is_picture_key(key: &[u8]) -> bool {
    key.eq_ignore_ascii_case(b"METADATA_BLOCK_PICTURE") || key.eq_ignore_ascii_case(b"COVERART")
}

#[cfg(feature = "vorbis_comments")]
#[test]
fn test_is_picture_key() {
    // Test uppercase
    assert!(is_picture_key(b"METADATA_BLOCK_PICTURE"));
    assert!(is_picture_key(b"COVERART"));

    // Test mixed-case
    assert!(is_picture_key(b"MetaData_Block_Picture"));
    assert!(is_picture_key(b"COverArT"));

    // Test lowercase
    assert!(is_picture_key(b"metadata_block_picture"));
    assert!(is_picture_key(b"coverart"));

    // Test false suffix
    assert!(!is_picture_key(b"METADATA_BLOCK_PICTURE_EXTRA"));
    assert!(!is_picture_key(b"COVERART-OTHER"));
    assert!(!is_picture_key(b"NOT_A_PICTURE"));
}
