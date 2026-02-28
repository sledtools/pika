use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct JitterBuffer<T> {
    max_frames: usize,
    target_frames: usize,
    min_target_frames: usize,
    max_target_frames: usize,
    adaptive: bool,
    arrival_jitter_ema: f32,
    underflow_boost: usize,
    frames: VecDeque<T>,
    dropped: u64,
    underflows: u64,
    playout_started: bool,
}

impl<T> JitterBuffer<T> {
    pub fn new(max_frames: usize) -> Self {
        Self::with_target(max_frames, 1)
    }

    pub fn with_target(max_frames: usize, target_frames: usize) -> Self {
        let max_frames = max_frames.max(1);
        let target_frames = target_frames.clamp(1, max_frames);
        Self {
            max_frames,
            target_frames,
            min_target_frames: target_frames,
            max_target_frames: target_frames,
            adaptive: false,
            arrival_jitter_ema: 0.0,
            underflow_boost: 0,
            frames: VecDeque::new(),
            dropped: 0,
            underflows: 0,
            playout_started: false,
        }
    }

    pub fn with_adaptive_target(
        max_frames: usize,
        initial_target_frames: usize,
        min_target_frames: usize,
        max_target_frames: usize,
    ) -> Self {
        let max_frames = max_frames.max(1);
        let min_target_frames = min_target_frames.clamp(1, max_frames);
        let max_target_frames = max_target_frames.clamp(min_target_frames, max_frames);
        let target_frames = initial_target_frames.clamp(min_target_frames, max_target_frames);

        Self {
            max_frames,
            target_frames,
            min_target_frames,
            max_target_frames,
            adaptive: true,
            arrival_jitter_ema: 0.0,
            underflow_boost: 0,
            frames: VecDeque::new(),
            dropped: 0,
            underflows: 0,
            playout_started: false,
        }
    }

    pub fn push(&mut self, frame: T) -> bool {
        self.frames.push_back(frame);
        let mut dropped = false;
        while self.frames.len() > self.max_frames {
            self.frames.pop_front();
            self.dropped += 1;
            dropped = true;
        }
        dropped
    }

    pub fn pop(&mut self) -> Option<T> {
        self.frames.pop_front()
    }

    pub fn pop_for_playout(&mut self) -> Option<T> {
        if !self.playout_started {
            if self.frames.len() < self.target_frames {
                return None;
            }
            self.playout_started = true;
        }

        match self.frames.pop_front() {
            Some(frame) => Some(frame),
            None => {
                self.playout_started = false;
                self.underflows = self.underflows.saturating_add(1);
                if self.adaptive {
                    self.underflow_boost = self
                        .underflow_boost
                        .saturating_add(3)
                        .min(self.max_target_frames);
                    self.target_frames = (self.target_frames.saturating_add(1))
                        .clamp(self.min_target_frames, self.max_target_frames);
                }
                None
            }
        }
    }

    pub fn observe_arrival_interval(&mut self, interval_ticks: u32) {
        if !self.adaptive {
            return;
        }

        let interval = interval_ticks.max(1) as f32;
        let jitter = (interval - 1.0).abs();
        if self.arrival_jitter_ema == 0.0 {
            self.arrival_jitter_ema = jitter;
        } else {
            self.arrival_jitter_ema = self.arrival_jitter_ema * 0.8 + jitter * 0.2;
        }

        let mut desired_target = self.min_target_frames + self.arrival_jitter_ema.ceil() as usize;
        if self.underflow_boost > 0 {
            desired_target = desired_target.saturating_add(1);
            self.underflow_boost = self.underflow_boost.saturating_sub(1);
        }
        desired_target = desired_target.clamp(self.min_target_frames, self.max_target_frames);

        if desired_target > self.target_frames {
            self.target_frames = self.target_frames.saturating_add(1).min(desired_target);
        } else if desired_target + 1 < self.target_frames {
            self.target_frames = self.target_frames.saturating_sub(1).max(desired_target);
        }
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn underflows(&self) -> u64 {
        self.underflows
    }

    pub fn target_frames(&self) -> usize {
        self.target_frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_drop_count_when_over_capacity() {
        let mut jb = JitterBuffer::new(2);
        assert!(!jb.push(1));
        assert!(!jb.push(2));
        assert!(jb.push(3));
        assert_eq!(jb.dropped(), 1);
        assert_eq!(jb.pop(), Some(2));
        assert_eq!(jb.pop(), Some(3));
    }

    #[test]
    fn playout_waits_for_prefill_target() {
        let mut jb = JitterBuffer::with_target(4, 2);
        assert!(!jb.push(10));
        assert_eq!(jb.pop_for_playout(), None);
        assert!(!jb.push(11));
        assert_eq!(jb.pop_for_playout(), Some(10));
        assert_eq!(jb.pop_for_playout(), Some(11));
    }

    #[test]
    fn underflow_resets_playout_until_refilled() {
        let mut jb = JitterBuffer::with_target(4, 2);
        assert!(!jb.push(1));
        assert!(!jb.push(2));
        assert_eq!(jb.pop_for_playout(), Some(1));
        assert_eq!(jb.pop_for_playout(), Some(2));
        assert_eq!(jb.pop_for_playout(), None);
        assert_eq!(jb.underflows(), 1);

        assert!(!jb.push(3));
        assert_eq!(jb.pop_for_playout(), None);
        assert!(!jb.push(4));
        assert_eq!(jb.pop_for_playout(), Some(3));
    }

    #[test]
    fn adaptive_target_grows_and_shrinks_with_jitter() {
        let mut jb = JitterBuffer::<i32>::with_adaptive_target(12, 2, 2, 8);
        assert_eq!(jb.target_frames(), 2);

        for _ in 0..10 {
            jb.observe_arrival_interval(4);
        }
        assert!(
            jb.target_frames() >= 4,
            "target should grow under jitter, got {}",
            jb.target_frames()
        );
        let grown_target = jb.target_frames();

        for _ in 0..40 {
            jb.observe_arrival_interval(1);
        }
        assert!(
            jb.target_frames() < grown_target,
            "target should shrink when jitter stabilizes, grew={} now={}",
            grown_target,
            jb.target_frames()
        );
        assert!(jb.target_frames() >= 2);
    }

    #[test]
    fn adaptive_target_bumps_on_underflow() {
        let mut jb = JitterBuffer::with_adaptive_target(8, 2, 2, 6);
        assert!(!jb.push(1));
        assert!(!jb.push(2));
        assert_eq!(jb.pop_for_playout(), Some(1));
        assert_eq!(jb.pop_for_playout(), Some(2));

        let before = jb.target_frames();
        assert_eq!(jb.pop_for_playout(), None);
        assert!(
            jb.target_frames() > before,
            "underflow should boost target: before={before} after={}",
            jb.target_frames()
        );
    }
}
