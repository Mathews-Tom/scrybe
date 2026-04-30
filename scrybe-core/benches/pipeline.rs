// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Pipeline-stage micro-benchmarks.
//!
//! Tracks the four metrics named in `docs/system-design.md` §11 ("Bench
//! gate") and `.docs/development-plan.md` §7.3.4: VAD throughput,
//! resample throughput, chunker throughput, encoder realtime factor.
//! End-to-end realtime factor and Whisper realtime factor are gated
//! behind the `whisper-local` feature and run on the self-hosted Tier-3
//! Mac runner; they are not part of this scaffold.
//!
//! CI invokes `cargo bench --no-run` to verify the benches compile on
//! every PR (cheap, deterministic, runner-agnostic). The absolute
//! throughput numbers and the >10% regression gate run on the Tier-3
//! Mac because GitHub-hosted runners share noisy CPUs and would emit
//! false regressions.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::expect_used,
    clippy::unwrap_used
)]

use std::sync::Arc;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use scrybe_core::pipeline::{
    resample_linear, Chunker, ChunkerConfig, Encoder, EncoderConfig, EnergyVad, NullEncoder, Vad,
};
use scrybe_core::types::{AudioFrame, FrameSource};

const SAMPLE_RATE: u32 = 16_000;
const FRAME_SAMPLES: usize = 1_600;

fn speech_samples(n: usize, freq_hz: f32) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            (t * freq_hz * std::f32::consts::TAU).sin()
        })
        .collect()
}

fn synthetic_frame(timestamp_ns: u64, samples: Vec<f32>) -> AudioFrame {
    AudioFrame {
        samples: Arc::from(samples),
        channels: 1,
        sample_rate: SAMPLE_RATE,
        timestamp_ns,
        source: FrameSource::Mic,
    }
}

fn bench_vad(c: &mut Criterion) {
    let mut group = c.benchmark_group("vad");
    for &len in &[160_usize, 1_600, 16_000] {
        let frame = synthetic_frame(0, speech_samples(len, 440.0));
        group.throughput(Throughput::Elements(len as u64));
        group.bench_with_input(
            BenchmarkId::new("energy_vad/decide", len),
            &frame,
            |b, frame| {
                let mut vad = EnergyVad::default();
                b.iter(|| vad.decide(frame));
            },
        );
    }
    group.finish();
}

fn bench_resample(c: &mut Criterion) {
    let mut group = c.benchmark_group("resample");
    for &(src, dst) in &[(48_000_u32, 16_000_u32), (44_100, 16_000), (16_000, 16_000)] {
        let samples = speech_samples(src as usize, 1_000.0);
        group.throughput(Throughput::Elements(samples.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("linear", format!("{src}_to_{dst}")),
            &samples,
            |b, samples| b.iter(|| resample_linear(samples, src, dst).expect("resample bench")),
        );
    }
    group.finish();
}

fn bench_chunker(c: &mut Criterion) {
    let mut group = c.benchmark_group("chunker");
    for &frames_per_run in &[10_usize, 50, 200] {
        group.throughput(Throughput::Elements(
            (frames_per_run * FRAME_SAMPLES) as u64,
        ));
        group.bench_with_input(
            BenchmarkId::new("speech_burst", frames_per_run),
            &frames_per_run,
            |b, &frames_per_run| {
                b.iter(|| {
                    let mut chunker = Chunker::new(
                        ChunkerConfig::default(),
                        EnergyVad::default(),
                        FrameSource::Mic,
                    );
                    let mut count = 0_usize;
                    let mut sink = |_c| count += 1;
                    for i in 0..frames_per_run {
                        let ts = (i as u64 * FRAME_SAMPLES as u64 * 1_000_000_000)
                            / u64::from(SAMPLE_RATE);
                        let frame = synthetic_frame(ts, speech_samples(FRAME_SAMPLES, 440.0));
                        chunker.push(frame, &mut sink);
                    }
                    chunker.finish(&mut sink);
                    count
                });
            },
        );
    }
    group.finish();
}

fn bench_encoder(c: &mut Criterion) {
    let mut group = c.benchmark_group("encoder");
    let cfg = EncoderConfig {
        sample_rate: SAMPLE_RATE,
        channels: 1,
        bitrate_bps: 32_000,
        page_interval: Duration::from_millis(100),
    };
    let samples_per_page = (SAMPLE_RATE as usize) / 10;
    let block = vec![0.5_f32; samples_per_page];
    for &page_count in &[1_usize, 8, 64] {
        group.throughput(Throughput::Elements((samples_per_page * page_count) as u64));
        group.bench_with_input(
            BenchmarkId::new("null_encoder/push_pcm", page_count),
            &page_count,
            |b, &page_count| {
                b.iter(|| {
                    let mut enc = NullEncoder::new(cfg);
                    let mut total = 0_usize;
                    for _ in 0..page_count {
                        total += enc.push_pcm(&block).expect("encoder bench").len();
                    }
                    total += enc.finish().expect("encoder bench").len();
                    total
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    name = pipeline_benches;
    config = Criterion::default().sample_size(20);
    targets = bench_vad, bench_resample, bench_chunker, bench_encoder
);
criterion_main!(pipeline_benches);
