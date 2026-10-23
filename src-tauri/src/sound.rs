//! Checked RIFF parsing. A WAV extension can contain PCM, ADPCM or xWMA.
use serde::Serialize;
use std::ops::Range;

pub const MAX_SOUND_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Serialize)]
pub struct SoundInfo {
    pub encoding: String,
    pub channels: u16,
    pub sample_rate: u32,
    pub duration: f64,
    pub duration_estimated: bool,
    pub bytes: usize,
}

pub struct Sound {
    pub bytes: Vec<u8>,
    pub format: Range<usize>,
    pub data: Range<usize>,
    pub packets: Vec<u32>,
    pub frames: u32,
    pub block_frames: u32,
    pub info: SoundInfo,
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

pub fn parse(bytes: Vec<u8>) -> Result<Sound, String> {
    if bytes.len() < 12 || bytes.len() as u64 > MAX_SOUND_BYTES || &bytes[..4] != b"RIFF" {
        return Err("This sound is not a supported RIFF audio file.".into());
    }
    let form = &bytes[8..12];
    if form != b"WAVE" && form != b"XWMA" {
        return Err("Unsupported RIFF audio type.".into());
    }
    let end = (u32_at(&bytes, 4) as usize)
        .checked_add(8)
        .ok_or("Invalid RIFF size.")?;
    if end > bytes.len() || end < 12 {
        return Err("The sound's RIFF container is truncated.".into());
    }
    let mut offset = 12;
    let mut format = None;
    let mut data = None;
    let mut fact = None;
    let mut packets = Vec::new();
    while offset + 8 <= end {
        let size = u32_at(&bytes, offset + 4) as usize;
        let start = offset + 8;
        let next = start.checked_add(size).ok_or("Invalid audio chunk size.")?;
        if next > end {
            return Err("An audio chunk extends beyond the RIFF container.".into());
        }
        match &bytes[offset..offset + 4] {
            b"fmt " => {
                if format.is_some() || !(16..=256).contains(&size) {
                    return Err("Invalid or repeated audio format chunk.".into());
                }
                format = Some(start..next);
            }
            b"data" => {
                if data.is_some() || size == 0 {
                    return Err("Empty or repeated audio data chunk.".into());
                }
                data = Some(start..next);
            }
            b"fact" if size >= 4 => fact = Some(u32_at(&bytes, start)),
            b"dpds" => {
                if size == 0 || size % 4 != 0 || size > 4 * 1024 * 1024 {
                    return Err("Invalid xWMA packet table.".into());
                }
                packets = bytes[start..next]
                    .chunks_exact(4)
                    .map(|part| u32::from_le_bytes(part.try_into().unwrap()))
                    .collect();
            }
            _ => {}
        }
        offset = next + (size & 1);
    }
    let format = format.ok_or("Missing audio format chunk.")?;
    let data = data.ok_or("Missing audio data chunk.")?;
    let fmt = &bytes[format.clone()];
    let tag = u16_at(fmt, 0);
    let channels = u16_at(fmt, 2);
    let rate = u32_at(fmt, 4);
    let align = u16_at(fmt, 12) as u32;
    let bits = u16_at(fmt, 14);
    if !(1..=2).contains(&channels) || !(8000..=192000).contains(&rate) || align == 0 {
        return Err("Unsupported channel count, sample rate or block alignment.".into());
    }
    if fmt.len() >= 18 && 18 + u16_at(fmt, 16) as usize > fmt.len() {
        return Err("The audio format extension is truncated.".into());
    }
    let size = data.len() as u64;
    let (encoding, frames, block_frames, estimated) = match tag {
        1 if form == b"WAVE" && matches!(bits, 8 | 16 | 24 | 32) => {
            if align != u32::from(channels) * u32::from(bits / 8) || size % u64::from(align) != 0 {
                return Err("Invalid PCM block alignment.".into());
            }
            ("PCM", size / u64::from(align), 1, false)
        }
        2 if form == b"WAVE" && bits == 4 && fmt.len() >= 22 => {
            let samples = u16_at(fmt, 18) as u32;
            let coefficients = u16_at(fmt, 20) as usize;
            if coefficients == 0
                || coefficients > 256
                || 22 + coefficients * 4 > fmt.len()
                || align < 7 * u32::from(channels)
                || samples < 2
                || samples != (align - 7 * u32::from(channels)) * 2 / u32::from(channels) + 2
            {
                return Err("Invalid Microsoft ADPCM format.".into());
            }
            // Only complete compressed blocks are offered; never synthesize missing data.
            if size % u64::from(align) != 0 {
                return Err("The Microsoft ADPCM data ends in a partial block.".into());
            }
            let padded = size / u64::from(align) * u64::from(samples);
            let exact_count = fact.is_some_and(|count| u64::from(count) == padded);
            ("Microsoft ADPCM", padded, samples, !exact_count)
        }
        0x161 | 0x162 if form == b"XWMA" => {
            if packets.is_empty()
                // An encoder padding packet may add no decoded frames.
                || packets.windows(2).any(|pair| pair[1] < pair[0])
                || size % packets.len() as u64 != 0
                || size / packets.len() as u64 != u64::from(align)
            {
                return Err("The xWMA packet table does not match its audio data.".into());
            }
            let decoded = u64::from(*packets.last().unwrap());
            let width = u64::from(channels) * 2;
            if decoded % width != 0 {
                return Err("Invalid xWMA decoded sample count.".into());
            }
            ("xWMA", decoded / width, 1, true)
        }
        _ => return Err(format!("Unsupported audio encoding: 0x{tag:04X}.")),
    };
    let frames = u32::try_from(frames).map_err(|_| "The sound is too long.")?;
    if frames == 0 {
        return Err("The sound contains no sample frames.".into());
    }
    let info = SoundInfo {
        encoding: encoding.into(),
        channels,
        sample_rate: rate,
        duration: f64::from(frames) / f64::from(rate),
        duration_estimated: estimated,
        bytes: bytes.len(),
    };
    Ok(Sound {
        bytes,
        format,
        data,
        packets,
        frames,
        block_frames,
        info,
    })
}
