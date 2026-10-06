//! Turns streamed model text into TTS-sized utterances.
//!
//! Latency matters most for the *first* utterance, so it is released at the
//! first clause boundary once it is long enough to sound natural; later ones
//! are released at sentence boundaries, which gives the TTS better prosody.
//! Markdown is stripped because it is spoken, not read.

#[derive(Debug, Clone)]
pub struct SpeechChunker {
    buf: String,
    emitted: usize,
    /// Minimum characters before the first clause-level split.
    pub first_min: usize,
    /// Force a split at a word boundary beyond this length.
    pub max_len: usize,
}

impl Default for SpeechChunker {
    fn default() -> Self {
        Self { buf: String::new(), emitted: 0, first_min: 28, max_len: 260 }
    }
}

const ABBREVIATIONS: &[&str] = &["e.g", "i.e", "etc", "vs", "mr", "mrs", "ms", "dr", "fig", "approx", "no"];

impl SpeechChunker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buf.push_str(text);
        let mut out = Vec::new();
        while let Some(cut) = self.next_cut() {
            let chunk: String = self.buf.drain(..cut).collect();
            if let Some(c) = clean(&chunk) {
                self.emitted += 1;
                out.push(c);
            }
        }
        out
    }

    pub fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.buf);
        let c = clean(&rest);
        if c.is_some() {
            self.emitted += 1;
        }
        c
    }

    fn next_cut(&self) -> Option<usize> {
        let b = &self.buf;
        let chars: Vec<(usize, char)> = b.char_indices().collect();
        for (k, &(i, c)) in chars.iter().enumerate() {
            let next = chars.get(k + 1).map(|x| x.1);
            // a boundary needs to see the following character to be sure
            let Some(next) = next else { break };
            let end = i + c.len_utf8();
            let sentence_end = matches!(c, '.' | '!' | '?' | '…')
                && (next.is_whitespace() || next == '"' || next == ')')
                && !is_abbreviation(&b[..i])
                && !(c == '.' && is_list_number(&b[..i]));
            let newline = c == '\n' && end > 1;
            let clause = self.emitted == 0
                && matches!(c, ',' | ';' | ':' | '—')
                && next.is_whitespace()
                && end >= self.first_min;
            if (sentence_end || newline || clause) && !b[..end].trim().is_empty() {
                return Some(end);
            }
            if end >= self.max_len && c.is_whitespace() {
                return Some(end);
            }
        }
        None
    }
}

fn is_abbreviation(before: &str) -> bool {
    let word: String = before
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '.')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let w = word.to_ascii_lowercase();
    ABBREVIATIONS.contains(&w.as_str()) || (w.len() == 1 && w.chars().all(|c| c.is_alphabetic()))
}

/// "1." at the start of a line is a list marker, not a sentence.
fn is_list_number(before: &str) -> bool {
    let line = before.rsplit('\n').next().unwrap_or(before).trim();
    !line.is_empty() && line.len() <= 3 && line.chars().all(|c| c.is_ascii_digit())
}

/// Strip markdown and collapse whitespace; `None` if nothing speakable.
pub fn clean(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        let l = line.trim_start();
        let l = l.trim_start_matches('#').trim_start();
        let l = l
            .strip_prefix("- ")
            .or_else(|| l.strip_prefix("* "))
            .or_else(|| l.strip_prefix("• "))
            .unwrap_or(l);
        out.push_str(l);
        out.push(' ');
    }
    let out: String = out.chars().filter(|c| !matches!(c, '*' | '`' | '_' | '#')).collect();
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    out.chars().any(|c| c.is_alphanumeric()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&str]) -> Vec<String> {
        let mut c = SpeechChunker::new();
        let mut out: Vec<String> = chunks.iter().flat_map(|s| c.push(s)).collect();
        out.extend(c.finish());
        out
    }

    #[test]
    fn first_chunk_released_at_clause() {
        let mut c = SpeechChunker::new();
        assert!(c.push("This is the architecture diagram").is_empty());
        let out = c.push(", and it shows how requests flow.");
        assert_eq!(out, vec!["This is the architecture diagram,"]);
    }

    #[test]
    fn later_chunks_wait_for_sentences() {
        let out = run(&["Short one. ", "Then a longer piece, with a comma, ", "ending here. Last"]);
        assert_eq!(out, vec!["Short one.", "Then a longer piece, with a comma, ending here.", "Last"]);
    }

    #[test]
    fn decimals_and_abbreviations_do_not_split() {
        let out = run(&["Version 3.5 is faster, e.g. for images. Done."]);
        assert_eq!(out, vec!["Version 3.5 is faster, e.g. for images.", "Done."]);
    }

    #[test]
    fn markdown_is_stripped() {
        let out = run(&["## Overview\n", "- **Gateway** routes `traffic`.\n"]);
        assert_eq!(out, vec!["Overview", "Gateway routes traffic."]);
    }

    #[test]
    fn list_numbers_are_not_sentences() {
        let out = run(&["Steps:\n1. Open the menu.\n2. Click save."]);
        assert_eq!(out, vec!["Steps:", "1. Open the menu.", "2. Click save."]);
    }

    #[test]
    fn needs_lookahead_before_cutting() {
        let mut c = SpeechChunker::new();
        assert!(c.push("Hello world.").is_empty());
        assert_eq!(c.push(" Next"), vec!["Hello world."]);
    }

    #[test]
    fn very_long_run_on_is_split() {
        let s = "word ".repeat(120);
        let out = run(&[&s]);
        assert!(out.len() >= 2);
        assert!(out.iter().all(|c| c.len() <= 265));
    }

    #[test]
    fn whitespace_only_is_dropped() {
        assert!(run(&["  \n ", "**"]).is_empty());
    }
}
