//! Keeps speech audio and visual annotations in the order the model wrote
//! them.
//!
//! TTS requests for successive sentences run concurrently and can complete out
//! of order. The sequencer releases items strictly in stream order, so a box
//! that the model wrote *before* "…and this is the database" appears exactly
//! when that sentence starts playing — not when the network happened to
//! deliver it.

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq)]
pub enum Released<A, M> {
    Audio { seq: u64, text: String, audio: A },
    Marker(M),
}

enum Slot<A, M> {
    Speech { seq: u64, text: String, audio: Option<Option<A>> },
    Marker(M),
}

pub struct Sequencer<A, M> {
    queue: VecDeque<Slot<A, M>>,
    next_seq: u64,
}

impl<A, M> Default for Sequencer<A, M> {
    fn default() -> Self {
        Self { queue: VecDeque::new(), next_seq: 0 }
    }
}

impl<A, M> Sequencer<A, M> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a sentence that is being synthesized; returns its sequence id.
    pub fn push_speech(&mut self, text: String) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.queue.push_back(Slot::Speech { seq, text, audio: None });
        seq
    }

    pub fn push_marker(&mut self, m: M) {
        self.queue.push_back(Slot::Marker(m));
    }

    /// Audio for `seq` arrived (`None` = synthesis failed; the text is skipped
    /// but following markers are still released).
    pub fn fulfill(&mut self, seq: u64, audio: Option<A>) {
        for s in self.queue.iter_mut() {
            if let Slot::Speech { seq: s2, audio: slot, .. } = s {
                if *s2 == seq {
                    *slot = Some(audio);
                    return;
                }
            }
        }
    }

    /// Everything at the head of the queue that may now be played.
    pub fn drain_ready(&mut self) -> Vec<Released<A, M>> {
        let mut out = Vec::new();
        loop {
            match self.queue.front() {
                Some(Slot::Marker(_)) => {
                    if let Some(Slot::Marker(m)) = self.queue.pop_front() {
                        out.push(Released::Marker(m));
                    }
                }
                Some(Slot::Speech { audio: Some(_), .. }) => {
                    if let Some(Slot::Speech { seq, text, audio: Some(a) }) = self.queue.pop_front() {
                        if let Some(audio) = a {
                            out.push(Released::Audio { seq, text, audio });
                        }
                    }
                }
                _ => break,
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Barge-in: drop everything not yet released.
    pub fn clear(&mut self) {
        self.queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type S = Sequencer<&'static str, &'static str>;

    #[test]
    fn markers_before_first_speech_release_immediately() {
        let mut s = S::new();
        s.push_marker("box");
        s.push_speech("hello".into());
        assert_eq!(s.drain_ready(), vec![Released::Marker("box")]);
    }

    #[test]
    fn out_of_order_audio_is_released_in_order() {
        let mut s = S::new();
        let a = s.push_speech("one".into());
        s.push_marker("m1");
        let b = s.push_speech("two".into());
        s.push_marker("m2");
        s.fulfill(b, Some("B"));
        assert!(s.drain_ready().is_empty());
        s.fulfill(a, Some("A"));
        assert_eq!(
            s.drain_ready(),
            vec![
                Released::Audio { seq: a, text: "one".into(), audio: "A" },
                Released::Marker("m1"),
                Released::Audio { seq: b, text: "two".into(), audio: "B" },
                Released::Marker("m2"),
            ]
        );
        assert!(s.is_empty());
    }

    #[test]
    fn failed_synthesis_does_not_block_markers() {
        let mut s = S::new();
        let a = s.push_speech("one".into());
        s.push_marker("m");
        s.fulfill(a, None);
        assert_eq!(s.drain_ready(), vec![Released::Marker("m")]);
    }

    #[test]
    fn clear_drops_pending() {
        let mut s = S::new();
        let a = s.push_speech("one".into());
        s.clear();
        s.fulfill(a, Some("A"));
        assert!(s.drain_ready().is_empty());
    }
}
