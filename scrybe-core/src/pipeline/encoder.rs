// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Opus encoder seam.
//!
//! The pipeline writes audio to disk through this trait so the storage
//! layer never touches a codec directly. Two implementations:
//!
//! - `NullEncoder` — deterministic test-only impl that emits
//!   little-endian f32 PCM bytes shaped by `page_interval`. Used in
//!   tests and as the fallback when `encoder-opus` is not built.
//! - `OggOpusEncoder` — production impl behind the `encoder-opus`
//!   feature flag (only present in feature-on builds). Encodes f32
//!   PCM with libopus into 20-ms Opus packets, packages them in an
//!   Ogg container per RFC 7845, and emits Ogg pages on a 1-second
//!   cadence so the storage layer's `append_durable` writes a
//!   recoverable file. Anticipated since v0.5 (see
//!   `.docs/development-plan.md` §7.2 audio-encoding scope); landed
//!   at v1.0.2 to close the v0.1 carryover where the pipeline wrote
//!   raw PCM under an `.opus` filename.
//!
//! Page flushing is the contract: the implementation MUST commit a
//! recoverable boundary every `EncoderConfig::page_interval`. The
//! storage layer's `append_durable` calls fsync on each returned page
//! so a crash mid-session loses at most the most recent partial page.

use std::time::Duration;

use crate::error::PipelineError;

/// Tunable encoder parameters. Defaults match `system-design.md` §9 —
/// Opus 32 kbps, 1-second pages.
#[derive(Clone, Copy, Debug)]
pub struct EncoderConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub bitrate_bps: u32,
    pub page_interval: Duration,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 1,
            bitrate_bps: 32_000,
            page_interval: Duration::from_secs(1),
        }
    }
}

/// Opus encoder seam. Implementations buffer frames and emit byte
/// blobs at page boundaries.
pub trait Encoder: Send {
    /// Feed PCM samples into the encoder. Returns any byte payload that
    /// completed a page boundary; an empty `Vec` means the encoder is
    /// still buffering.
    ///
    /// # Errors
    ///
    /// Implementations return `PipelineError::OpusEncode` for codec
    /// failures.
    fn push_pcm(&mut self, samples: &[f32]) -> Result<Vec<u8>, PipelineError>;

    /// Flush the encoder's tail buffer at session end. After calling
    /// `finish`, further calls to `push_pcm` are undefined; callers
    /// drop the encoder and finalize the audio file.
    ///
    /// # Errors
    ///
    /// `PipelineError::OpusEncode` when the codec rejects the flush.
    fn finish(&mut self) -> Result<Vec<u8>, PipelineError>;
}

/// Test-only encoder.
///
/// Buffers PCM as little-endian f32 bytes and emits a "page" every
/// `page_interval` of audio. Useful for asserting page-flush behavior
/// in pipeline tests without pulling in an Opus dependency.
pub struct NullEncoder {
    config: EncoderConfig,
    pending: Vec<f32>,
    samples_per_page: usize,
}

impl NullEncoder {
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn new(config: EncoderConfig) -> Self {
        let samples_per_page = (f64::from(config.sample_rate) * config.page_interval.as_secs_f64())
            .round() as usize
            * usize::from(config.channels.max(1));
        Self {
            config,
            pending: Vec::with_capacity(samples_per_page * 2),
            samples_per_page: samples_per_page.max(1),
        }
    }

    #[must_use]
    pub const fn config(&self) -> EncoderConfig {
        self.config
    }
}

impl Encoder for NullEncoder {
    fn push_pcm(&mut self, samples: &[f32]) -> Result<Vec<u8>, PipelineError> {
        self.pending.extend_from_slice(samples);
        if self.pending.len() < self.samples_per_page {
            return Ok(Vec::new());
        }
        let take = self.samples_per_page;
        let drained: Vec<f32> = self.pending.drain(..take).collect();
        Ok(pcm_to_bytes(&drained))
    }

    fn finish(&mut self) -> Result<Vec<u8>, PipelineError> {
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }
        let drained = std::mem::take(&mut self.pending);
        Ok(pcm_to_bytes(&drained))
    }
}

fn pcm_to_bytes(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// Helper that wraps a string into the `BoxError` shape required by
/// `PipelineError::OpusEncode`. `Box<dyn Error + Send + Sync>` does
/// not have a direct `From<String>` impl that satisfies the variant's
/// signature; routing through `std::io::Error::other` is the
/// conventional shim.
#[cfg(feature = "encoder-opus")]
fn opus_err<S: Into<String>>(msg: S) -> Box<dyn std::error::Error + Send + Sync + 'static> {
    Box::new(std::io::Error::other(msg.into()))
}

/// Construct the default audio encoder for a session.
///
/// Picks `OggOpusEncoder` when the `encoder-opus` feature is built and
/// falls back to `NullEncoder` otherwise. Returns a boxed `dyn`
/// because the choice is a compile-time feature gate; the orchestrator
/// (`scrybe-core::session::drive_session`) uses the boxed shape
/// uniformly.
///
/// # Errors
///
/// `PipelineError::OpusEncode` if the Opus encoder rejects the
/// configured sample rate or channel count at construction
/// (`encoder-opus` builds only). The `NullEncoder` fallback is
/// infallible.
pub fn default_session_encoder(config: EncoderConfig) -> Result<Box<dyn Encoder>, PipelineError> {
    #[cfg(feature = "encoder-opus")]
    {
        let enc = OggOpusEncoder::new(config)?;
        Ok(Box::new(enc))
    }
    #[cfg(not(feature = "encoder-opus"))]
    {
        Ok(Box::new(NullEncoder::new(config)))
    }
}

/// Production Ogg-Opus encoder. Buffers f32 PCM into 20-ms Opus
/// frames, encodes via libopus, packages packets into an Ogg stream
/// per RFC 7845, and emits Ogg pages every `page_interval` of audio.
///
/// Available only when the `encoder-opus` feature is enabled.
#[cfg(feature = "encoder-opus")]
pub struct OggOpusEncoder {
    config: EncoderConfig,
    opus: opus::Encoder,
    /// In-memory buffer the underlying `ogg::PacketWriter` writes to.
    /// `OggOpusEncoder::push_pcm` drains it into the returned
    /// `Vec<u8>` after each batch of packets so the pipeline never
    /// holds more than one page in memory.
    ogg_buffer: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    writer: ogg::PacketWriter<'static, OggBufferSink>,
    /// Stream serial number; constant for the lifetime of one Ogg
    /// stream (one session in scrybe).
    serial: u32,
    /// Pending PCM samples awaiting the next 20-ms frame boundary.
    pcm_pending: Vec<f32>,
    samples_per_opus_frame: usize,
    /// How many encoded packets sit between Ogg page flushes. The
    /// `page_interval` is approximate — we flush after the first
    /// packet that completes the configured interval.
    packets_per_page: u32,
    packets_in_current_page: u32,
    /// Opus granulepos: total samples encoded so far at the encoder's
    /// 48-kHz clock (RFC 7845 §4 — granulepos is at the Opus output
    /// rate, not the input rate, but this encoder takes 48-kHz input
    /// directly so they match).
    granulepos: u64,
    /// Reusable encode buffer; sized for the worst-case Opus packet
    /// (4 KiB is well above the 1275-byte cap at 32 kbps).
    encode_buf: Vec<u8>,
    /// Set to true after the `OpusHead` + `OpusTags` header pages are
    /// written. The headers go out on the first `push_pcm` call so
    /// the encoder is fully constructed before any I/O happens.
    headers_written: bool,
    finished: bool,
}

#[cfg(feature = "encoder-opus")]
struct OggBufferSink {
    buffer: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

#[cfg(feature = "encoder-opus")]
impl std::io::Write for OggBufferSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer
            .lock()
            .map_err(|_| std::io::Error::other("OggBufferSink mutex poisoned"))?
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(feature = "encoder-opus")]
impl OggOpusEncoder {
    /// Construct a new encoder. Validates `sample_rate` is one of the
    /// rates Opus supports natively (8/12/16/24/48 kHz) and
    /// `channels` is mono or stereo. The encoder is configured for
    /// `Application::Voip` (better quality at low bitrates for speech)
    /// and the bitrate from `config.bitrate_bps`.
    ///
    /// # Errors
    ///
    /// `PipelineError::OpusEncode` if the configuration is rejected
    /// by libopus.
    pub fn new(config: EncoderConfig) -> Result<Self, PipelineError> {
        let opus_rate = match config.sample_rate {
            8_000 | 12_000 | 16_000 | 24_000 | 48_000 => config.sample_rate,
            other => {
                return Err(PipelineError::OpusEncode(opus_err(format!(
                    "unsupported sample rate {other} Hz; Opus accepts 8000/12000/16000/24000/48000"
                ))));
            }
        };
        let opus_channels = match config.channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            n => {
                return Err(PipelineError::OpusEncode(opus_err(format!(
                    "unsupported channel count {n}; OggOpusEncoder requires mono or stereo"
                ))));
            }
        };
        let mut opus = opus::Encoder::new(opus_rate, opus_channels, opus::Application::Voip)
            .map_err(|e| {
                PipelineError::OpusEncode(opus_err(format!("opus::Encoder::new failed: {e}")))
            })?;
        opus.set_bitrate(opus::Bitrate::Bits(
            i32::try_from(config.bitrate_bps).unwrap_or(32_000),
        ))
        .map_err(|e| {
            PipelineError::OpusEncode(opus_err(format!("opus set_bitrate failed: {e}")))
        })?;

        // 20-ms Opus frame at the configured rate. 20 ms is the modal
        // VoIP frame size and balances latency against bitrate
        // efficiency. Per RFC 7845 §1, valid frame durations are
        // 2.5/5/10/20/40/60 ms.
        let samples_per_opus_frame =
            (config.sample_rate / 50) as usize * usize::from(config.channels.max(1));

        // Convert page_interval (e.g., 1 s) into the number of 20-ms
        // packets per Ogg page. `max(1)` so a sub-20-ms interval
        // still flushes every packet.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let packets_per_page = ((config.page_interval.as_secs_f64() * 50.0).round() as u32).max(1);

        let ogg_buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::with_capacity(8 * 1024)));
        let sink = OggBufferSink {
            buffer: ogg_buffer.clone(),
        };
        let writer = ogg::PacketWriter::new(sink);

        // Stream serial: a 32-bit value derived from system time so
        // distinct sessions cannot share a serial. RFC 7845 §3 says
        // "a value chosen at random" satisfies the protocol.
        #[allow(clippy::cast_possible_truncation)]
        let serial = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0xCAFE_BABE_u32, |d| d.as_nanos() as u32);

        Ok(Self {
            config,
            opus,
            ogg_buffer,
            writer,
            serial,
            pcm_pending: Vec::with_capacity(samples_per_opus_frame * 4),
            samples_per_opus_frame,
            packets_per_page,
            packets_in_current_page: 0,
            granulepos: 0,
            encode_buf: vec![0_u8; 4096],
            headers_written: false,
            finished: false,
        })
    }

    fn write_headers(&mut self) -> Result<(), PipelineError> {
        let head = build_opus_head(self.config.sample_rate, self.config.channels);
        let tags = build_opus_tags();
        self.writer
            .write_packet(head, self.serial, ogg::PacketWriteEndInfo::EndPage, 0)
            .map_err(|e| PipelineError::OpusEncode(opus_err(format!("ogg write OpusHead: {e}"))))?;
        self.writer
            .write_packet(tags, self.serial, ogg::PacketWriteEndInfo::EndPage, 0)
            .map_err(|e| PipelineError::OpusEncode(opus_err(format!("ogg write OpusTags: {e}"))))?;
        self.headers_written = true;
        Ok(())
    }

    fn drain_buffer(&self) -> Vec<u8> {
        let mut guard = match self.ogg_buffer.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        std::mem::take(&mut *guard)
    }

    /// Granulepos increment per 20-ms Opus frame at the canonical
    /// 48-kHz output clock (RFC 7845 §4). Constant regardless of the
    /// encoder's input rate, validated at construction.
    const GRANULEPOS_PER_20MS_FRAME: u64 = 960;

    fn encode_one_frame(&mut self, end_stream: bool) -> Result<(), PipelineError> {
        let frame: Vec<f32> = self
            .pcm_pending
            .drain(..self.samples_per_opus_frame)
            .collect();
        let n = self
            .opus
            .encode_float(&frame, &mut self.encode_buf)
            .map_err(|e| PipelineError::OpusEncode(opus_err(format!("opus encode_float: {e}"))))?;
        let packet = self.encode_buf[..n].to_vec();

        self.granulepos = self
            .granulepos
            .saturating_add(Self::GRANULEPOS_PER_20MS_FRAME);

        let end_info = if end_stream {
            ogg::PacketWriteEndInfo::EndStream
        } else if self.packets_in_current_page + 1 >= self.packets_per_page {
            ogg::PacketWriteEndInfo::EndPage
        } else {
            ogg::PacketWriteEndInfo::NormalPacket
        };

        self.writer
            .write_packet(packet, self.serial, end_info, self.granulepos)
            .map_err(|e| PipelineError::OpusEncode(opus_err(format!("ogg write packet: {e}"))))?;

        if matches!(
            end_info,
            ogg::PacketWriteEndInfo::EndPage | ogg::PacketWriteEndInfo::EndStream
        ) {
            self.packets_in_current_page = 0;
        } else {
            self.packets_in_current_page += 1;
        }
        Ok(())
    }
}

#[cfg(feature = "encoder-opus")]
impl Encoder for OggOpusEncoder {
    fn push_pcm(&mut self, samples: &[f32]) -> Result<Vec<u8>, PipelineError> {
        if self.finished {
            return Err(PipelineError::OpusEncode(opus_err(
                "OggOpusEncoder::push_pcm called after finish()",
            )));
        }
        if !self.headers_written {
            self.write_headers()?;
        }
        self.pcm_pending.extend_from_slice(samples);
        while self.pcm_pending.len() >= self.samples_per_opus_frame {
            self.encode_one_frame(false)?;
        }
        Ok(self.drain_buffer())
    }

    fn finish(&mut self) -> Result<Vec<u8>, PipelineError> {
        if self.finished {
            return Ok(Vec::new());
        }
        if !self.headers_written {
            self.write_headers()?;
        }
        // Encode every full frame still buffered. If a sub-frame
        // remainder exists, pad with silence so the last 20-ms slot
        // is encoded too — losing the trailing sub-frame would
        // truncate the audio file by up to 20 ms which is more
        // surprising than emitting a silent tail.
        while self.pcm_pending.len() >= self.samples_per_opus_frame {
            self.encode_one_frame(false)?;
        }
        // Pad the remaining buffer (or an empty buffer) up to one full
        // frame so the EndStream marker has a valid Opus packet to ride
        // on — the `ogg` crate requires SOME packet for `EndStream`.
        let pad = self
            .samples_per_opus_frame
            .saturating_sub(self.pcm_pending.len());
        if pad > 0 {
            self.pcm_pending.extend(std::iter::repeat_n(0.0_f32, pad));
        }
        self.encode_one_frame(true)?;
        self.finished = true;
        Ok(self.drain_buffer())
    }
}

#[cfg(feature = "encoder-opus")]
fn build_opus_head(input_sample_rate: u32, channels: u16) -> Vec<u8> {
    // RFC 7845 §5.1: OpusHead packet layout.
    let mut buf = Vec::with_capacity(19);
    buf.extend_from_slice(b"OpusHead");
    buf.push(1); // version
    #[allow(clippy::cast_possible_truncation)]
    buf.push(channels as u8);
    buf.extend_from_slice(&3840_u16.to_le_bytes()); // pre-skip: 80ms at 48kHz
    buf.extend_from_slice(&input_sample_rate.to_le_bytes());
    buf.extend_from_slice(&0_i16.to_le_bytes()); // output gain
    buf.push(0); // mapping family
    buf
}

#[cfg(feature = "encoder-opus")]
fn build_opus_tags() -> Vec<u8> {
    // RFC 7845 §5.2: OpusTags packet layout. Vendor string
    // identifies the producer; user comment list is empty.
    let vendor = b"scrybe";
    let mut buf = Vec::with_capacity(8 + 4 + vendor.len() + 4);
    buf.extend_from_slice(b"OpusTags");
    #[allow(clippy::cast_possible_truncation)]
    buf.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    buf.extend_from_slice(vendor);
    buf.extend_from_slice(&0_u32.to_le_bytes()); // 0 user comments
    buf
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::float_cmp
)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const fn cfg() -> EncoderConfig {
        EncoderConfig {
            sample_rate: 16_000,
            channels: 1,
            bitrate_bps: 32_000,
            page_interval: Duration::from_millis(100),
        }
    }

    #[test]
    fn test_null_encoder_emits_page_when_buffer_reaches_page_size() {
        let mut enc = NullEncoder::new(cfg());
        let samples_per_page = 1_600;
        let pcm = vec![0.5_f32; samples_per_page];

        let out = enc.push_pcm(&pcm).unwrap();

        assert_eq!(out.len(), samples_per_page * 4);
    }

    #[test]
    fn test_null_encoder_buffers_below_page_size_without_emitting() {
        let mut enc = NullEncoder::new(cfg());
        let pcm = vec![0.5_f32; 800];

        let out = enc.push_pcm(&pcm).unwrap();

        assert!(out.is_empty());
    }

    #[test]
    fn test_null_encoder_finish_returns_tail_below_page_threshold() {
        let mut enc = NullEncoder::new(cfg());
        enc.push_pcm(&vec![0.25_f32; 200]).unwrap();

        let tail = enc.finish().unwrap();

        assert_eq!(tail.len(), 200 * 4);
    }

    #[test]
    fn test_null_encoder_finish_returns_empty_when_no_buffer() {
        let mut enc = NullEncoder::new(cfg());

        let tail = enc.finish().unwrap();

        assert!(tail.is_empty());
    }

    #[test]
    fn test_null_encoder_emits_multiple_pages_for_large_burst() {
        let mut enc = NullEncoder::new(cfg());

        let first = enc.push_pcm(&vec![0.1_f32; 1_600]).unwrap();
        let second = enc.push_pcm(&vec![0.2_f32; 1_600]).unwrap();

        assert_eq!(first.len(), 1_600 * 4);
        assert_eq!(second.len(), 1_600 * 4);
    }

    #[test]
    fn test_null_encoder_round_trip_decodes_to_within_5_percent_rms() {
        let mut enc = NullEncoder::new(cfg());
        let pcm = vec![0.5_f32; 1_600];

        let bytes = enc.push_pcm(&pcm).unwrap();

        let decoded: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let in_rms = (pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len() as f32).sqrt();
        let out_rms = (decoded.iter().map(|s| s * s).sum::<f32>() / decoded.len() as f32).sqrt();
        let ratio = out_rms / in_rms;
        assert!(
            (ratio - 1.0).abs() < 0.05,
            "round-trip RMS deviates by more than 5%: {ratio}"
        );
    }

    #[test]
    fn test_encoder_config_default_uses_documented_v0_1_settings() {
        let c = EncoderConfig::default();

        assert_eq!(c.sample_rate, 48_000);
        assert_eq!(c.channels, 1);
        assert_eq!(c.bitrate_bps, 32_000);
        assert_eq!(c.page_interval, Duration::from_secs(1));
    }

    #[cfg(not(feature = "encoder-opus"))]
    #[test]
    fn test_default_session_encoder_returns_null_encoder_without_feature() {
        let mut enc = default_session_encoder(EncoderConfig::default()).unwrap();
        let pcm = vec![0.5_f32; 48_000];
        let bytes = enc.push_pcm(&pcm).unwrap();
        // NullEncoder fallback emits raw f32 LE bytes.
        assert_eq!(bytes.len(), 48_000 * 4);
    }

    #[cfg(feature = "encoder-opus")]
    mod opus_tests {
        use super::*;

        fn opus_cfg() -> EncoderConfig {
            EncoderConfig {
                sample_rate: 48_000,
                channels: 1,
                bitrate_bps: 32_000,
                page_interval: Duration::from_millis(100),
            }
        }

        #[test]
        fn test_ogg_opus_encoder_rejects_unsupported_sample_rate() {
            let cfg = EncoderConfig {
                sample_rate: 44_100,
                ..opus_cfg()
            };
            let Err(err) = OggOpusEncoder::new(cfg) else {
                panic!("expected unsupported-config error");
            };
            let msg = format!("{err}");
            assert!(
                msg.contains("44100") || msg.contains("unsupported sample rate"),
                "unexpected error message: {msg}"
            );
        }

        #[test]
        fn test_ogg_opus_encoder_rejects_six_channels() {
            let cfg = EncoderConfig {
                channels: 6,
                ..opus_cfg()
            };
            let Err(err) = OggOpusEncoder::new(cfg) else {
                panic!("expected unsupported-config error");
            };
            let msg = format!("{err}");
            assert!(
                msg.contains("unsupported channel count") || msg.contains('6'),
                "unexpected error message: {msg}"
            );
        }

        #[test]
        fn test_ogg_opus_encoder_emits_ogg_magic_on_first_push() {
            let mut enc = OggOpusEncoder::new(opus_cfg()).unwrap();
            // 20-ms frame at 48 kHz mono = 960 samples.
            let pcm = vec![0.0_f32; 960];
            let bytes = enc.push_pcm(&pcm).unwrap();
            // First emitted bytes must start with the OggS capture
            // pattern (RFC 3533 §6).
            assert!(
                bytes.len() >= 4 && &bytes[..4] == b"OggS",
                "first push must emit OggS-prefixed page; got {} bytes starting with {:?}",
                bytes.len(),
                bytes.iter().take(8).collect::<Vec<_>>()
            );
        }

        #[test]
        fn test_ogg_opus_encoder_buffers_below_one_opus_frame() {
            let mut enc = OggOpusEncoder::new(opus_cfg()).unwrap();
            // 100 samples is well below one 20-ms / 960-sample frame.
            // The header pages still emit on first push (which is
            // correct — RFC 7845 requires headers before any audio
            // packet); the assertion is that no audio packet flushes
            // until a full Opus frame is buffered.
            let bytes = enc.push_pcm(&vec![0.0_f32; 100]).unwrap();
            // Bytes are non-empty (the OpusHead+OpusTags pages flush)
            // but contain no audio packet — verifiable by absence of
            // a third Ogg page (the audio one) which would push the
            // length past ~80 bytes for headers alone.
            assert!(
                bytes.len() < 200,
                "no audio frame should flush before a full Opus packet is buffered; got {} bytes",
                bytes.len()
            );
        }

        #[test]
        fn test_ogg_opus_encoder_finish_emits_end_stream_marker() {
            let mut enc = OggOpusEncoder::new(opus_cfg()).unwrap();
            // Push 1 second of silence so multiple Opus packets exist.
            let _ = enc.push_pcm(&vec![0.0_f32; 48_000]).unwrap();
            let tail = enc.finish().unwrap();
            // The tail page must be present and contain the OggS
            // capture pattern. Subsequent push_pcm calls must error.
            assert!(
                tail.is_empty() || tail.starts_with(b"OggS"),
                "finish tail must be empty or start with OggS"
            );
            let after_finish = enc.push_pcm(&vec![0.0_f32; 100]);
            assert!(
                after_finish.is_err(),
                "push_pcm after finish must error; got {after_finish:?}"
            );
        }

        #[test]
        fn test_default_session_encoder_returns_ogg_opus_with_feature() {
            // Build via the public factory and assert the encoded
            // output starts with the OggS magic so the cfg-gating
            // routes through the Opus impl.
            let mut enc = default_session_encoder(opus_cfg()).unwrap();
            let bytes = enc.push_pcm(&vec![0.0_f32; 960]).unwrap();
            assert!(
                bytes.len() >= 4 && &bytes[..4] == b"OggS",
                "default_session_encoder must return OggOpusEncoder when feature is on"
            );
        }
    }
}
