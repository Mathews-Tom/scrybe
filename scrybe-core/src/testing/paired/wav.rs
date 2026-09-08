// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Strict RIFF PCM16 decoding. Unknown metadata chunks are skipped, but
//! truncated containers, duplicate audio/format chunks, and inconsistent
//! frame layouts fail rather than producing a partial audio measurement.

pub(super) struct DecodedPcm16Wav {
    pub(super) samples: Vec<f32>,
    pub(super) sample_rate: u32,
    pub(super) channels: u16,
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

pub(super) fn decode_pcm16_wav(bytes: &[u8]) -> Result<DecodedPcm16Wav, String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("missing RIFF/WAVE header".into());
    }
    if u64::from(u32_at(bytes, 4)) + 8 != bytes.len() as u64 {
        return Err("RIFF container length does not match the complete file".into());
    }
    let mut cursor = 12;
    let mut format = None;
    let mut data = None;
    while cursor < bytes.len() {
        if bytes.len() - cursor < 8 {
            return Err("truncated RIFF chunk header".into());
        }
        let id = &bytes[cursor..cursor + 4];
        let len = u32_at(bytes, cursor + 4) as usize;
        cursor += 8;
        if len > bytes.len() - cursor {
            return Err("truncated RIFF chunk payload".into());
        }
        let body = &bytes[cursor..cursor + len];
        match id {
            b"fmt " => {
                if format.is_some() || body.len() < 16 {
                    return Err("duplicate or truncated 'fmt ' chunk".into());
                }
                let channels = u16_at(body, 2);
                let sample_rate = u32_at(body, 4);
                let block_align = u16_at(body, 12);
                if u16_at(body, 0) != 1 || u16_at(body, 14) != 16 {
                    return Err(
                        "only canonical PCM format code 1 with 16-bit samples is accepted".into(),
                    );
                }
                if channels == 0
                    || u32::from(block_align) != u32::from(channels) * 2
                    || u64::from(u32_at(body, 8)) != u64::from(sample_rate) * u64::from(block_align)
                {
                    return Err("inconsistent PCM byte rate or frame alignment".into());
                }
                format = Some((channels, sample_rate, block_align));
            }
            b"data" if data.is_some() => return Err("duplicate 'data' chunk".into()),
            b"data" => data = Some(body),
            _ => {}
        }
        cursor += len;
        if !len.is_multiple_of(2) {
            if cursor == bytes.len() {
                return Err("missing RIFF chunk padding byte".into());
            }
            cursor += 1;
        }
    }
    let (channels, sample_rate, block_align) = format.ok_or("missing 'fmt ' chunk")?;
    let data = data.ok_or("missing 'data' chunk")?;
    if data.is_empty() {
        return Err("decoded to zero samples".into());
    }
    if data.len() % usize::from(block_align) != 0 {
        return Err("'data' length is not a whole PCM frame".into());
    }
    let samples = data
        .chunks_exact(2)
        .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32768.0)
        .collect();
    Ok(DecodedPcm16Wav {
        samples,
        sample_rate,
        channels,
    })
}
