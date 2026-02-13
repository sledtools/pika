use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct JitterBuffer<T> {
    max_frames: usize,
    frames: VecDeque<T>,
}

impl<T> JitterBuffer<T> {
    pub fn new(max_frames: usize) -> Self {
        Self {
            max_frames,
            frames: VecDeque::new(),
        }
    }

    pub fn push(&mut self, frame: T) {
        self.frames.push_back(frame);
        while self.frames.len() > self.max_frames {
            self.frames.pop_front();
        }
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
}
