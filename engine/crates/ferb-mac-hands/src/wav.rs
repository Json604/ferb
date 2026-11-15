//! Mono float32 WAV written as samples arrive (ferb/act/mac/audio.py:35 `WavWriter`): no recording is held in
//! memory, silence the app didn't send is written as zeros so transcript times match the media's, and the header's
//! sizes are filled in at close. A new file only: an existing one is never replaced.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const SAMPLE_RATE: u32 = 16000;
static ZEROS: [u8; 65536] = [0; 65536];

/// What a closed file holds.
#[derive(Debug, Clone, PartialEq)]
pub struct WavFacts {
    pub frames: u64,
    pub sample_rate: u32,
    pub audio_s: f64,
    pub peak: f64,
}

struct Inner {
    file: Option<File>,
    frames: u64,
    peak: f64,
    rate: u32,
}

pub struct WavWriter {
    pub path: PathBuf,
    inner: Mutex<Inner>,
}

/// Python's `round(x, ndigits)` (correctly rounded on the double's exact value, half to even).
pub fn py_round(x: f64, ndigits: usize) -> f64 {
    format!("{x:.ndigits$}").parse().unwrap_or(x)
}

/// Python's `round(x)` to an int, half to even.
pub fn py_round_int(x: f64) -> i64 {
    x.round_ties_even() as i64
}

impl WavWriter {
    pub fn create(path: &Path) -> std::io::Result<WavWriter> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o666)
            .open(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        f.write_all(&[0u8; 44])?;
        Ok(WavWriter {
            path: path.to_path_buf(),
            inner: Mutex::new(Inner {
                file: Some(f),
                frames: 0,
                peak: 0.0,
                rate: 0,
            }),
        })
    }

    /// Append mono float32 samples (native byte order); `at` (seconds from the file's start) zero-fills any silence
    /// before them.
    pub fn write(&self, samples: &[u8], rate: u32, at: Option<f64>) -> std::io::Result<()> {
        let usable = samples.len() - samples.len() % 4;
        let values: Vec<f32> = samples[..usable]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_ne_bytes(*c))
            .collect();
        let mut g = self.inner.lock().expect("lock");
        let Inner {
            file,
            frames,
            peak,
            rate: r,
        } = &mut *g;
        let Some(f) = file.as_mut() else {
            return Ok(());
        };
        if *r == 0 {
            *r = rate;
        }
        let gap: i64 = match at {
            Some(at) => py_round_int(at * *r as f64) - *frames as i64,
            None => 0,
        };
        let mut left = 4 * gap;
        while left > 0 {
            let piece = left.min(ZEROS.len() as i64) as usize;
            f.write_all(&ZEROS[..piece])?;
            left -= piece as i64;
        }
        *frames += gap.max(0) as u64;
        f.write_all(&samples[..usable])?;
        *frames += values.len() as u64;
        for v in values {
            let a = (v as f64).abs();
            if a > *peak || a.is_nan() {
                *peak = if a.is_nan() { *peak } else { a };
            }
        }
        Ok(())
    }

    pub fn close(&self) -> WavFacts {
        let mut g = self.inner.lock().expect("lock");
        if let Some(mut f) = g.file.take() {
            let rate = if g.rate == 0 { SAMPLE_RATE } else { g.rate };
            let data = (g.frames * 4) as u32;
            let mut head = Vec::with_capacity(44);
            head.extend_from_slice(b"RIFF");
            head.extend_from_slice(&(36 + data).to_le_bytes());
            head.extend_from_slice(b"WAVE");
            head.extend_from_slice(b"fmt ");
            head.extend_from_slice(&16u32.to_le_bytes());
            head.extend_from_slice(&3u16.to_le_bytes());
            head.extend_from_slice(&1u16.to_le_bytes());
            head.extend_from_slice(&rate.to_le_bytes());
            head.extend_from_slice(&(rate * 4).to_le_bytes());
            head.extend_from_slice(&4u16.to_le_bytes());
            head.extend_from_slice(&32u16.to_le_bytes());
            head.extend_from_slice(b"data");
            head.extend_from_slice(&data.to_le_bytes());
            let _ = f.seek(SeekFrom::Start(0)).and_then(|_| f.write_all(&head));
        }
        let rate = if g.rate == 0 { SAMPLE_RATE } else { g.rate };
        WavFacts {
            frames: g.frames,
            sample_rate: rate,
            audio_s: g.frames as f64 / rate as f64,
            peak: py_round(g.peak, 4),
        }
    }
}
