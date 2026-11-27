use super::item::ApeItem;
use super::ApeTag;
use crate::ape::constants::{APE_PREAMBLE, INVALID_KEYS};
use crate::ape::header::ApeHeader;
use crate::error::Result;
use crate::macros::{decode_err, err, try_vec};
use crate::tag::item::ItemValue;

use std::io::{Read, Seek, SeekFrom};

use byteorder::{LittleEndian, ReadBytesExt};

pub(crate) fn read_ape_tag<R>(data: &mut R, header: ApeHeader) -> Result<ApeTag>
where
	R: Read + Seek,
{
	let mut tag = ApeTag::default();
	let mut remaining_size = header.size;

	for _ in 0..header.item_count {
		if remaining_size < 11 {
			break;
		}

		let value_size = data.read_u32::<LittleEndian>()?;
		if value_size > remaining_size {
			err!(SizeMismatch);
		}

		remaining_size -= 4;
		let flags = data.read_u32::<LittleEndian>()?;

		let mut key = Vec::new();
		let mut key_char = data.read_u8()?;

		while key_char != 0 {
			key.push(key_char);
			key_char = data.read_u8()?;
		}

		let key = String::from_utf8(key)
			.map_err(|_| decode_err!(APE, "APE tag item contains a non UTF-8 key"))?;

		if INVALID_KEYS.contains(&&*key.to_uppercase()) {
			decode_err!(@BAIL APE, "APE tag item contains an illegal key");
		}

		let read_only = (flags & 1) == 1;
		let item_type = (flags >> 1) & 3;

		if value_size == 0 || key.len() < 2 || key.len() > 255 {
			log::debug!("APE: Encountered invalid item key ({})", key);
			continue;
		}

		let mut value = try_vec![0; value_size as usize];
		data.read_exact(&mut value)?;

		let parsed_value = match item_type {
			0 => ItemValue::Text(String::from_utf8(value).map_err(|_| {
				decode_err!(APE, "Failed to convert text item into a UTF-8 string")
			})?),
			1 => ItemValue::Binary(value),
			2 => ItemValue::Locator(String::from_utf8(value).map_err(|_| {
				decode_err!(APE, "Failed to convert locator item into a UTF-8 string")
			})?),
			_ => decode_err!(@BAIL APE, "APE tag item contains an invalid item type"),
		};

		let mut item = ApeItem::new(key, parsed_value)?;

		if read_only {
			item.read_only = true;
		}

		tag.insert(item);
	}

	// Skip over footer
	data.seek(SeekFrom::Current(32))?;

	Ok(tag)
}

/// Reads only bounded text and locator items for the media library.
pub(crate) fn read_ape_tag_for_scan<R>(
	data: &mut R,
	header: ApeHeader,
	text_remaining: &mut usize,
) -> Result<ApeTag>
where
	R: Read + Seek,
{
	let start = data.stream_position()?;
	let items_end = start.checked_add(u64::from(header.items_size)).ok_or_else(|| {
		decode_err!(APE, "APE tag items exceed the file")
	})?;
	let footer_end = items_end.checked_add(32).ok_or_else(|| {
		decode_err!(APE, "APE tag footer exceeds the file")
	})?;
	let end = data.seek(SeekFrom::End(0))?;
	if footer_end > end {
		err!(SizeMismatch);
	}
	data.seek(SeekFrom::Start(start))?;

	let mut tag = ApeTag::default();
	let mut remaining = u64::from(header.items_size);
	let mut key = [0u8; 255];
	let mut bytes = [0u8; 4 * 1024];
	for _ in 0..header.item_count {
		if remaining < 9 {
			err!(SizeMismatch);
		}
		let value_len = data.read_u32::<LittleEndian>()?;
		let flags = data.read_u32::<LittleEndian>()?;
		remaining -= 8;
		let value_len = u64::from(value_len);
		let key_room = remaining.checked_sub(value_len).ok_or_else(|| {
			decode_err!(APE, "APE tag item exceeds the tag length")
		})?;
		let mut key_len = 0;
		loop {
			if key_len as u64 >= key_room {
				err!(SizeMismatch);
			}
			let byte = data.read_u8()?;
			if byte == 0 {
				break;
			}
			if key_len == key.len() {
				decode_err!(@BAIL APE, "APE tag item key exceeds its format limit");
			}
			key[key_len] = byte;
			key_len += 1;
		}
		remaining -= key_len as u64 + 1 + value_len;
		let key = &key[..key_len];
		let item_type = (flags >> 1) & 3;
		let is_art = key.get(..10).is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"COVER ART "));
		let value_len_usize = value_len as usize;
		if value_len == 0
			|| !(2..=255).contains(&key_len)
			|| !matches!(item_type, 0 | 2)
			|| is_art
			|| value_len_usize > bytes.len()
			|| key_len + value_len_usize > *text_remaining
		{
			data.seek(SeekFrom::Current(value_len as i64))?;
			continue;
		}
		let name = std::str::from_utf8(key).map_err(|_| {
			decode_err!(APE, "APE tag item contains a non UTF-8 key")
		})?;
		if INVALID_KEYS.iter().any(|invalid| invalid.eq_ignore_ascii_case(name)) {
			decode_err!(@BAIL APE, "APE tag item contains an illegal key");
		}
		data.read_exact(&mut bytes[..value_len_usize])?;
		let value = std::str::from_utf8(&bytes[..value_len_usize]).map_err(|_| {
			decode_err!(APE, "APE tag item contains non UTF-8 text")
		})?;
		*text_remaining -= key_len + value.len();
		let value = if item_type == 0 { ItemValue::Text(value.to_owned()) }
			else { ItemValue::Locator(value.to_owned()) };
		let mut item = ApeItem::new(name.to_owned(), value)?;
		item.read_only = (flags & 1) == 1;
		tag.insert(item);
	}
	data.seek(SeekFrom::Start(items_end))?;
	let mut footer = [0u8; 32];
	data.read_exact(&mut footer)?;
	if &footer[..8] != APE_PREAMBLE {
		decode_err!(@BAIL APE, "APE tag footer does not match the declared tag length");
	}
	Ok(tag)
}

