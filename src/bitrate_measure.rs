use std::collections::VecDeque;
use std::time::Instant;
use str0m::Bitrate;

struct BitrateSample {
    data_size: u32,
    time: Instant,
}

/// Measures the average bitrate over time
pub struct BitrateMeasure {
    samples: VecDeque<BitrateSample>,
    samples_count: usize,
}

impl BitrateMeasure {
    pub fn new(samples_count: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(samples_count),
            samples_count,
        }
    }

    pub fn push(&mut self, data_size: u32) {
        if self.samples.len() == self.samples_count {
            self.samples.pop_front();
        }
        self.samples.push_back(BitrateSample {
            data_size,
            time: Instant::now(),
        });
    }

    pub fn bitrate(&self) -> Bitrate {
        let since_earliest = match self.samples.get(0) {
            Some(sample) => sample.time.elapsed().as_secs_f64(),
            None => return Bitrate::ZERO,
        };
        let sum: u32 = self.samples.iter().map(|s| s.data_size).sum();
        let byterate = (sum as f64) / since_earliest;
        Bitrate::from(byterate * 8.0)
    }
}
