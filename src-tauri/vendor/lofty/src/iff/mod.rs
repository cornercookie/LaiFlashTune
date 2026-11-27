//! WAV/AIFF specific items
pub mod aiff;
pub(crate) mod chunk;
pub mod wav;
#[cfg(all(feature = "id3v2", feature = "riff_info_list", feature = "aiff_text_chunks"))]
pub mod scan;
