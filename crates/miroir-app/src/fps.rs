//! Frames per second over the last second.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct FpsCounter {
    ticks: VecDeque<Instant>,
}

impl FpsCounter {
    /// Records a frame received at `now`.
    pub fn tick(&mut self, now: Instant) {
        self.ticks.push_back(now);
    }

    /// Number of frames received during the second that ends at `now`.
    pub fn fps(&mut self, now: Instant) -> u32 {
        let second = Duration::from_secs(1);
        while self.ticks.front().is_some_and(|t| now.saturating_duration_since(*t) >= second) {
            self.ticks.pop_front();
        }
        self.ticks.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_the_frames_of_the_last_second() {
        let start = Instant::now();
        let mut counter = FpsCounter::default();
        // 3 seconds at 60 fps.
        for i in 0..180 {
            counter.tick(start + Duration::from_secs_f64(i as f64 / 60.0));
        }
        assert_eq!(counter.fps(start + Duration::from_secs_f64(179.0 / 60.0)), 60);
    }

    #[test]
    fn drops_to_zero_when_frames_stop() {
        let start = Instant::now();
        let mut counter = FpsCounter::default();
        counter.tick(start);
        assert_eq!(counter.fps(start), 1);
        assert_eq!(counter.fps(start + Duration::from_secs(2)), 0);
    }
}
