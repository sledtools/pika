use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct JitterBuffer<T> {
    max_frames: usize,
    frames: VecDeque<T>,
    dropped: u64,
}

impl<T> JitterBuffer<T> {
    pub fn new(max_frames: usize) -> Self {
        Self {
            max_frames,
            frames: VecDeque::new(),
            dropped: 0,
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

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
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
}
