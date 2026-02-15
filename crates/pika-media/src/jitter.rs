use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct JitterBuffer<T> {
    max_frames: usize,
    target_frames: usize,
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
                None
            }
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
}
