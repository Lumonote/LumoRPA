use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const TARGET_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFrame {
    pub samples: Vec<i16>,
}

impl AudioFrame {
    pub fn from_i16_interleaved(input: &[i16], channels: usize) -> Self {
        assert!(channels > 0, "channels must be non-zero");
        let samples = input
            .chunks_exact(channels)
            .map(|frame| {
                let sum: i64 = frame.iter().map(|&x| i64::from(x)).sum();
                (sum / channels as i64).clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
            })
            .collect();
        Self { samples }
    }
    pub fn from_f32_interleaved(input: &[f32], channels: usize) -> Self {
        assert!(channels > 0, "channels must be non-zero");
        let samples = input
            .chunks_exact(channels)
            .map(|frame| {
                let avg = frame.iter().map(|x| x.clamp(-1.0, 1.0)).sum::<f32>() / channels as f32;
                if avg <= -1.0 {
                    i16::MIN
                } else {
                    (avg * f32::from(i16::MAX)).round() as i16
                }
            })
            .collect();
        Self { samples }
    }
}

#[derive(Debug)]
pub struct PreRollBuffer {
    samples: VecDeque<i16>,
    capacity: usize,
}
impl PreRollBuffer {
    pub fn two_seconds() -> Self {
        Self::new((TARGET_SAMPLE_RATE * 2) as usize)
    }
    pub fn new(capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
        }
    }
    pub fn len(&self) -> usize {
        self.samples.len()
    }
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
    pub fn push(&mut self, frame: AudioFrame) {
        for sample in frame.samples {
            if self.samples.len() == self.capacity {
                self.samples.pop_front();
            }
            self.samples.push_back(sample);
        }
    }
    pub fn drain_after_wake(&mut self) -> AudioFrame {
        AudioFrame {
            samples: self.samples.drain(..).collect(),
        }
    }
    pub fn clear(&mut self) {
        for sample in &mut self.samples {
            *sample = 0;
        }
        self.samples.clear();
    }
}
