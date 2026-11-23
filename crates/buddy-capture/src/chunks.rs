//! WAV chunk publication and JSONL gap bookkeeping.
//!
//! Only the writer thread calls the mutating functions here, which keeps
//! publication serialized without a lock. Each chunk is staged under a unique
//! temporary name, synced, then renamed into its final position.

use crate::{CaptureError, TrackKind};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const HEADER_BYTES: usize = 44;

/// Metadata for one published chunk. `frames` is the number of interleaved
/// f32 values in the data chunk, which is what callers compare against the
/// configured chunk size (`48_000` frames × 2 channels = `96_000` values).
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkInfo {
    pub track: TrackKind,
    pub index: u32,
    pub path: PathBuf,
    pub frames: u32,
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapReason {
    Overrun,
    DeviceLoss,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GapMarker {
    /// The track that lost samples. Kept on disk so a Mic-only outage is not
    /// indistinguishable from a System-only outage during recovery.
    pub track: TrackKind,
    pub start_ms: u64,
    pub end_ms: u64,
    pub reason: GapReason,
}

fn track_stem(track: &TrackKind) -> &'static str {
    match track {
        TrackKind::System => "system",
        TrackKind::Mic => "mic",
    }
}

fn wav_header(
    values: usize,
    sample_rate: u32,
    channels: u16,
) -> Result<[u8; HEADER_BYTES], CaptureError> {
    // Every f32 is four bytes, and both sizes live in u32 header fields. Check
    // each step so an oversized chunk is rejected instead of wrapping in debug
    // builds or silently truncating the published header.
    let data_bytes = u32::try_from(values)
        .ok()
        .and_then(|values| values.checked_mul(4))
        .ok_or_else(|| {
            CaptureError::InvalidChunk(format!("{values} samples exceed the WAV data size limit"))
        })?;
    let riff_size = data_bytes.checked_add(36).ok_or_else(|| {
        CaptureError::InvalidChunk("WAV RIFF size exceeds the u32 header field".to_owned())
    })?;
    let block_align = channels.checked_mul(4).ok_or_else(|| {
        CaptureError::InvalidChunk(format!(
            "{channels} channels exceed the WAV block align limit"
        ))
    })?;
    let byte_rate = sample_rate
        .checked_mul(u32::from(block_align))
        .ok_or_else(|| {
            CaptureError::InvalidChunk(format!(
                "sample rate {sample_rate} with {channels} channels is too large"
            ))
        })?;
    let mut h = [0u8; HEADER_BYTES];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&riff_size.to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&3u16.to_le_bytes()); // WAVE_FORMAT_IEEE_FLOAT
    h[22..24].copy_from_slice(&channels.to_le_bytes());
    h[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    h[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    h[32..34].copy_from_slice(&block_align.to_le_bytes());
    h[34..36].copy_from_slice(&32u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    Ok(h)
}

// Directory fsync is durability hardening only: opening a directory fails on
// Windows, so keep this Unix-only and best-effort like session.rs.
#[cfg(unix)]
fn sync_dir(path: &Path) {
    if let Ok(dir) = fs::File::open(path) {
        let _ = dir.sync_all();
    }
}

/// Publish one interleaved IEEE-float chunk. The final path is visible only
/// after the staged file has been fully written and synced.
pub fn write_chunk(
    session_dir: &Path,
    track: TrackKind,
    index: u32,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<ChunkInfo, CaptureError> {
    if samples.is_empty() || sample_rate == 0 || channels == 0 {
        return Err(CaptureError::InvalidChunk(
            "chunk needs samples, a sample rate and channels".to_owned(),
        ));
    }
    if samples.len() % usize::from(channels) != 0 {
        return Err(CaptureError::InvalidChunk(format!(
            "chunk has {} samples for {channels} channels: incomplete interleaved frame",
            samples.len()
        )));
    }

    let audio_dir = session_dir.join("audio");
    fs::create_dir_all(&audio_dir)?;
    let file_name = format!("{}-{index:04}.wav", track_stem(&track));
    let final_path = audio_dir.join(&file_name);
    // A unique staging name keeps overlapping writes from truncating one
    // another's in-progress chunk.
    let tmp_path = audio_dir.join(format!(".{}.{}.tmp", file_name, Uuid::new_v4()));

    let header = wav_header(samples.len(), sample_rate, channels)?;
    let frames = u32::try_from(samples.len()).map_err(|_| {
        CaptureError::InvalidChunk(format!(
            "chunk has {} samples: frame count exceeds u32",
            samples.len()
        ))
    })?;
    let mut bytes = Vec::with_capacity(HEADER_BYTES + samples.len() * 4);
    bytes.extend_from_slice(&header);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }

    let publication = (|| -> Result<(), CaptureError> {
        let mut tmp = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        tmp.write_all(&bytes)?;
        tmp.sync_all()?;
        drop(tmp);
        fs::rename(&tmp_path, &final_path)?;
        Ok(())
    })();
    // A failed publication must not leak staging files that the chunk index
    // ignores; the original error is more useful than any cleanup failure.
    if let Err(error) = publication {
        let _ = fs::remove_file(&tmp_path);
        return Err(error);
    }
    #[cfg(unix)]
    sync_dir(&audio_dir);

    Ok(ChunkInfo {
        track,
        index,
        path: final_path,
        frames,
        sample_rate,
        channels,
    })
}

fn parse_chunk_name(name: &str) -> Option<(TrackKind, u32)> {
    let (stem, extension) = name.rsplit_once('.')?;
    if extension != "wav" {
        return None;
    }
    let (stem, digits) = stem.rsplit_once('-')?;
    if digits.len() < 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let track = match stem {
        "system" => TrackKind::System,
        "mic" => TrackKind::Mic,
        _ => return None,
    };
    Some((track, digits.parse().ok()?))
}

fn parse_chunk(path: &Path, track: TrackKind, index: u32) -> Result<ChunkInfo, CaptureError> {
    let bytes = fs::read(path)?;
    if bytes.len() < HEADER_BYTES || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(CaptureError::InvalidChunk(format!(
            "{}: missing RIFF/WAVE header",
            path.display()
        )));
    }

    let mut pos = 12usize;
    let mut sample_rate = None;
    let mut channels = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let start = pos + 8;
        let end = start.checked_add(size).ok_or_else(|| {
            CaptureError::InvalidChunk(format!("{}: chunk size overflow", path.display()))
        })?;
        if end > bytes.len() {
            return Err(CaptureError::InvalidChunk(format!(
                "{}: truncated chunk",
                path.display()
            )));
        }
        match id {
            b"fmt " if size >= 16 => {
                let format = u16::from_le_bytes(bytes[start..start + 2].try_into().unwrap());
                if format != 3 {
                    return Err(CaptureError::InvalidChunk(format!(
                        "{}: not IEEE float",
                        path.display()
                    )));
                }
                channels = Some(u16::from_le_bytes(
                    bytes[start + 2..start + 4].try_into().unwrap(),
                ));
                sample_rate = Some(u32::from_le_bytes(
                    bytes[start + 4..start + 8].try_into().unwrap(),
                ));
                let bits = u16::from_le_bytes(bytes[start + 14..start + 16].try_into().unwrap());
                if bits != 32 {
                    return Err(CaptureError::InvalidChunk(format!(
                        "{}: not 32-bit float",
                        path.display()
                    )));
                }
            }
            b"data" if size % 4 == 0 => {
                let (sample_rate, channels) = match (sample_rate, channels) {
                    (Some(rate), Some(channels)) if rate > 0 && channels > 0 => (rate, channels),
                    _ => {
                        return Err(CaptureError::InvalidChunk(format!(
                            "{}: missing or invalid fmt chunk",
                            path.display()
                        )))
                    }
                };
                if size % (usize::from(channels) * 4) != 0 {
                    return Err(CaptureError::InvalidChunk(format!(
                        "{}: incomplete interleaved frame",
                        path.display()
                    )));
                }
                return Ok(ChunkInfo {
                    track,
                    index,
                    path: path.to_owned(),
                    // `size` was read from a u32 field, so `size / 4` fits.
                    frames: (size / 4) as u32,
                    sample_rate,
                    channels,
                });
            }
            _ => {}
        }
        pos = end + (size % 2);
    }
    Err(CaptureError::InvalidChunk(format!(
        "{}: missing data chunk",
        path.display()
    )))
}

/// Validate the published chunks for a session. Files that are not chunks are
/// ignored (including temporary staging files), so an empty session yields an
/// empty index rather than an error.
pub fn read_chunk_index(session_dir: &Path) -> Result<Vec<ChunkInfo>, CaptureError> {
    let audio_dir = session_dir.join("audio");
    let entries = match fs::read_dir(&audio_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut chunks = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some((track, index)) = parse_chunk_name(&name.to_string_lossy()) else {
            continue;
        };
        chunks.push(parse_chunk(&entry.path(), track, index)?);
    }
    chunks.sort_by_key(|c| (track_stem(&c.track), c.index));
    Ok(chunks)
}

/// Append one gap line. The caller is the writer thread, so no lock is needed.
pub fn append_gap(session_dir: &Path, marker: &GapMarker) -> Result<(), CaptureError> {
    let audio_dir = session_dir.join("audio");
    fs::create_dir_all(&audio_dir)?;
    let mut line =
        serde_json::to_vec(marker).map_err(|err| CaptureError::InvalidChunk(err.to_string()))?;
    line.push(b'\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(audio_dir.join("gaps.jsonl"))?;
    file.write_all(&line)?;
    file.sync_all()?;
    Ok(())
}

pub fn read_gaps(session_dir: &Path) -> Result<Vec<GapMarker>, CaptureError> {
    let path = session_dir.join("audio/gaps.jsonl");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut gaps = Vec::new();
    for (line_no, line) in bytes.split(|&b| b == b'\n').enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let marker: GapMarker = serde_json::from_slice(line).map_err(|err| {
            CaptureError::InvalidChunk(format!("{} line {}: {}", path.display(), line_no + 1, err))
        })?;
        gaps.push(marker);
    }
    Ok(gaps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_rejects_a_data_size_that_would_truncate() {
        // One Gi f32 values would need 4 Gi of data, past the u32 header field.
        let error = wav_header(u32::MAX as usize, 48_000, 2).unwrap_err();
        assert!(matches!(error, CaptureError::InvalidChunk(_)), "{error}");
        // A usize too large even for u32 is rejected the same way.
        let error = wav_header(usize::MAX, 48_000, 2).unwrap_err();
        assert!(matches!(error, CaptureError::InvalidChunk(_)), "{error}");
    }

    #[test]
    fn wav_header_rejects_an_overwide_block_align() {
        // 16_384 * 4 no longer fits the u16 block-align field.
        let error = wav_header(16, 48_000, 16_384).unwrap_err();
        assert!(matches!(error, CaptureError::InvalidChunk(_)), "{error}");
    }

    #[test]
    fn wav_header_rejects_an_overflowing_byte_rate() {
        let error = wav_header(16, u32::MAX, 2).unwrap_err();
        assert!(matches!(error, CaptureError::InvalidChunk(_)), "{error}");
    }

    #[test]
    fn wav_header_accepts_valid_inputs() {
        let header = wav_header(96_000, 48_000, 2).unwrap();
        assert_eq!(&header[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(header[40..44].try_into().unwrap()),
            96_000 * 4
        );
        assert_eq!(u16::from_le_bytes(header[32..34].try_into().unwrap()), 8);
    }
}
