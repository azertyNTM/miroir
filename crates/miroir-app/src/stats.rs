//! What the status line shows: frames, and time spent per frame, over the last second.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Time spent on one frame.
#[derive(Clone, Copy)]
pub struct Timing {
    /// From the captured picture to the compressed data.
    pub encode: Duration,
    /// From the compressed data back to a picture.
    pub decode: Duration,
    /// Turning that picture into a screen texture.
    pub display: Duration,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Summary {
    /// Frames shown during the last second.
    pub fps: u32,
    /// Averages over those frames.
    pub encode: Duration,
    pub decode: Duration,
    pub display: Duration,
    /// Encoding plus decoding of the slowest of those frames.
    pub worst: Duration,
}

#[derive(Default)]
pub struct Stats {
    frames: VecDeque<(Instant, Timing)>,
}

impl Stats {
    /// Records a frame shown at `now`.
    pub fn record(&mut self, now: Instant, timing: Timing) {
        self.frames.push_back((now, timing));
    }

    /// Sums up the second that ends at `now`.
    pub fn summary(&mut self, now: Instant) -> Summary {
        let second = Duration::from_secs(1);
        while self
            .frames
            .front()
            .is_some_and(|(t, _)| now.saturating_duration_since(*t) >= second)
        {
            self.frames.pop_front();
        }
        let count = self.frames.len() as u32;
        if count == 0 {
            return Summary::default();
        }
        let average = |part: fn(&Timing) -> Duration| {
            self.frames
                .iter()
                .map(|(_, timing)| part(timing))
                .sum::<Duration>()
                / count
        };
        Summary {
            fps: count,
            encode: average(|timing| timing.encode),
            decode: average(|timing| timing.decode),
            display: average(|timing| timing.display),
            worst: self
                .frames
                .iter()
                .map(|(_, t)| t.encode + t.decode)
                .max()
                .unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing(encode: u64, decode: u64, display: u64) -> Timing {
        let ms = Duration::from_millis;
        Timing {
            encode: ms(encode),
            decode: ms(decode),
            display: ms(display),
        }
    }

    #[test]
    fn counts_the_frames_of_the_last_second() {
        let start = Instant::now();
        let mut stats = Stats::default();
        // 3 seconds at 60 fps.
        for i in 0..180 {
            stats.record(
                start + Duration::from_secs_f64(i as f64 / 60.0),
                timing(1, 1, 1),
            );
        }
        assert_eq!(
            stats
                .summary(start + Duration::from_secs_f64(179.0 / 60.0))
                .fps,
            60
        );
    }

    #[test]
    fn drops_to_zero_when_frames_stop() {
        let start = Instant::now();
        let mut stats = Stats::default();
        stats.record(start, timing(5, 5, 5));
        assert_eq!(stats.summary(start).fps, 1);
        assert_eq!(
            stats.summary(start + Duration::from_secs(2)),
            Summary::default()
        );
    }

    #[test]
    fn averages_each_step_and_keeps_the_slowest_frame() {
        let start = Instant::now();
        let ms = Duration::from_millis;
        let mut stats = Stats::default();
        // Too old to count by the time the summary is made.
        stats.record(start, timing(100, 100, 100));
        for (encode, decode) in [(2, 1), (4, 6), (6, 2)] {
            stats.record(start + ms(1500), timing(encode, decode, 3));
        }
        let expected = Summary {
            fps: 3,
            encode: ms(4),
            decode: ms(3),
            display: ms(3),
            worst: ms(10),
        };
        assert_eq!(stats.summary(start + ms(1600)), expected);
    }
}
