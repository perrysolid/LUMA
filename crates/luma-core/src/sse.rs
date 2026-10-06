//! Server-sent-events framing plus extraction of Gemini streaming deltas.

use serde_json::Value;

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    data: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed raw bytes; returns complete event payloads (`data:` joined).
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(std::mem::take(&mut self.data));
                }
            } else if let Some(d) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(d.strip_prefix(' ').unwrap_or(d));
            }
        }
        out
    }

    pub fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.buf);
        if !rest.is_empty() {
            self.push(&rest);
            self.push(b"\n");
        }
        (!self.data.is_empty()).then(|| std::mem::take(&mut self.data))
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct GeminiDelta {
    /// Visible answer text (thought summaries are excluded).
    pub text: String,
    pub finish_reason: Option<String>,
    pub block_reason: Option<String>,
    pub error: Option<String>,
}

pub fn parse_gemini_chunk(json: &str) -> GeminiDelta {
    let mut d = GeminiDelta::default();
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        d.error = Some("unparseable response chunk".into());
        return d;
    };
    if let Some(e) = v.get("error") {
        d.error = Some(e.get("message").and_then(Value::as_str).unwrap_or("unknown error").to_string());
        return d;
    }
    if let Some(r) = v.pointer("/promptFeedback/blockReason").and_then(Value::as_str) {
        d.block_reason = Some(r.to_string());
    }
    if let Some(c) = v.pointer("/candidates/0") {
        if let Some(parts) = c.pointer("/content/parts").and_then(Value::as_array) {
            for p in parts {
                if p.get("thought").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                if let Some(t) = p.get("text").and_then(Value::as_str) {
                    d.text.push_str(t);
                }
            }
        }
        d.finish_reason = c.get("finishReason").and_then(Value::as_str).map(str::to_string);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_split_across_reads() {
        let mut p = SseParser::new();
        assert!(p.push(b"data: {\"a\":").is_empty());
        assert_eq!(p.push(b"1}\r\n\r\ndata: {\"b\":2}\n\n"), vec!["{\"a\":1}", "{\"b\":2}"]);
        assert_eq!(p.push(b"data: {\"c\":3}"), Vec::<String>::new());
        assert_eq!(p.finish().as_deref(), Some("{\"c\":3}"));
    }

    #[test]
    fn gemini_text_skips_thoughts() {
        let d = parse_gemini_chunk(
            r#"{"candidates":[{"content":{"parts":[{"text":"plan","thought":true},{"text":"Hello "},{"text":"there"}]},"finishReason":"STOP"}]}"#,
        );
        assert_eq!(d.text, "Hello there");
        assert_eq!(d.finish_reason.as_deref(), Some("STOP"));
    }

    #[test]
    fn gemini_errors_and_blocks() {
        assert_eq!(
            parse_gemini_chunk(r#"{"error":{"code":429,"message":"Resource exhausted"}}"#).error.as_deref(),
            Some("Resource exhausted")
        );
        assert_eq!(
            parse_gemini_chunk(r#"{"promptFeedback":{"blockReason":"SAFETY"}}"#).block_reason.as_deref(),
            Some("SAFETY")
        );
        assert!(parse_gemini_chunk("nope").error.is_some());
    }
}
