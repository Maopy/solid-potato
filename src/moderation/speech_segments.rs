//! Pause-based utterances; classifier decisions are supplied by the speech reader.
use std::collections::VecDeque;

pub const FRAME_MS: usize = 10;
pub const FRAME_BYTES: usize = 16000 * FRAME_MS / 1000 * 2;

pub struct Frame {
    pub pcm: Vec<u8>,
    pub end_ms: i64,
}

pub struct Utterance {
    pub pcm: Vec<u8>,
    pub start_ms: i64,
    pub end_ms: i64,
}

struct Active {
    pcm: Vec<u8>,
    start_ms: i64,
    end_ms: i64,
    last_voice_bytes: usize,
    voiced_frames: usize,
    quiet_frames: usize,
}

pub struct Segmenter {
    pre: VecDeque<(Frame, bool)>,
    active: Option<Active>,
    pause_frames: usize,
    max_bytes: usize,
}

impl Segmenter {
    pub fn new(pause_ms: usize, max_ms: usize) -> Self {
        Self {
            pre: VecDeque::new(),
            active: None,
            pause_frames: pause_ms.div_ceil(FRAME_MS),
            max_bytes: max_ms / FRAME_MS * FRAME_BYTES,
        }
    }

    pub fn push(&mut self, frame: Frame, voice: bool) -> Option<Utterance> {
        if self.active.is_none() {
            self.pre.push_back((frame, voice));
            while self.pre.len() > 200 / FRAME_MS {
                self.pre.pop_front();
            }
            if !voice
                || self
                    .pre
                    .iter()
                    .rev()
                    .take(5)
                    .filter(|(_, voice)| *voice)
                    .count()
                    < 3
            {
                return None;
            }
            let start_ms = self.pre.front().unwrap().0.end_ms - FRAME_MS as i64;
            let mut active = Active {
                pcm: Vec::new(),
                start_ms,
                end_ms: start_ms,
                last_voice_bytes: 0,
                voiced_frames: 0,
                quiet_frames: 0,
            };
            for (frame, voice) in self.pre.drain(..) {
                Self::append(&mut active, frame, voice);
            }
            self.active = Some(active);
        } else {
            Self::append(self.active.as_mut().unwrap(), frame, voice);
        }
        let active = self.active.as_ref().unwrap();
        if active.quiet_frames >= self.pause_frames || active.pcm.len() >= self.max_bytes {
            self.finish()
        } else {
            None
        }
    }

    fn append(active: &mut Active, frame: Frame, voice: bool) {
        active.pcm.extend(frame.pcm);
        if voice {
            active.voiced_frames += 1;
            active.quiet_frames = 0;
            active.end_ms = frame.end_ms;
            active.last_voice_bytes = active.pcm.len();
        } else {
            active.quiet_frames += 1;
        }
    }

    pub fn finish(&mut self) -> Option<Utterance> {
        let mut active = self.active.take()?;
        self.pre.clear();
        if active.voiced_frames < 8 {
            return None;
        }
        // Keep a little acoustic tail, but timestamp the last speech frame.
        active.pcm.truncate(
            (active.last_voice_bytes + 80 / FRAME_MS * FRAME_BYTES).min(active.pcm.len()),
        );
        Some(Utterance {
            pcm: active.pcm,
            start_ms: active.start_ms,
            end_ms: active.end_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(
        segmenter: &mut Segmenter,
        index: &mut i64,
        count: usize,
        voice: bool,
    ) -> Vec<Utterance> {
        (0..count)
            .filter_map(|_| {
                *index += FRAME_MS as i64;
                segmenter.push(
                    Frame {
                        pcm: vec![if voice { 1 } else { 0 }; FRAME_BYTES],
                        end_ms: *index,
                    },
                    voice,
                )
            })
            .collect()
    }

    #[test]
    fn pauses_separate_sentences_and_timestamps_exclude_waiting_for_silence() {
        let mut s = Segmenter::new(600, 10000);
        let mut time = 0;
        assert!(feed(&mut s, &mut time, 20, false).is_empty());
        assert!(feed(&mut s, &mut time, 100, true).is_empty());
        assert!(feed(&mut s, &mut time, 59, false).is_empty());
        let first = feed(&mut s, &mut time, 1, false).pop().unwrap();
        assert_eq!(first.end_ms, 1200);
        assert_eq!(first.start_ms, 30);
        assert!(first.pcm.len() < 2 * 16000 * 2);
        feed(&mut s, &mut time, 100, true);
        let second = feed(&mut s, &mut time, 60, false).pop().unwrap();
        assert_eq!(second.end_ms, 2800);
    }

    #[test]
    fn brief_hesitations_stay_together_and_end_of_stream_flushes_last_words() {
        let mut s = Segmenter::new(600, 10000);
        let mut time = 0;
        feed(&mut s, &mut time, 100, true);
        assert!(feed(&mut s, &mut time, 30, false).is_empty());
        assert!(feed(&mut s, &mut time, 100, true).is_empty());
        assert_eq!(s.finish().unwrap().end_ms, 2300);
        assert!(s.finish().is_none());
    }

    #[test]
    fn long_sentence_waits_for_the_end_of_speech() {
        let mut s = Segmenter::new(600, 60000);
        let mut time = 0;
        assert!(feed(&mut s, &mut time, 1500, true).is_empty());
        assert!(feed(&mut s, &mut time, 59, false).is_empty());
        let utterance = feed(&mut s, &mut time, 1, false).pop().unwrap();
        assert_eq!(utterance.end_ms, 15000);
        assert!(s.finish().is_none());
    }

    #[test]
    fn non_speech_never_accumulates_and_continuous_speech_is_bounded() {
        let mut s = Segmenter::new(600, 10000);
        let mut time = 0;
        assert!(feed(&mut s, &mut time, 6000, false).is_empty());
        assert!(s.pre.len() <= 20);
        assert!(s.finish().is_none());
        let forced = feed(&mut s, &mut time, 1200, true);
        assert_eq!(forced.len(), 1);
        assert!(forced[0].pcm.len() <= 10 * 16000 * 2);
        assert!(s.finish().is_some());
    }
}
