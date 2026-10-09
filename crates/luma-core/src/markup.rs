//! Incremental parser for the model's output stream.
//!
//! The model streams natural speech with inline, self-closing visual tags:
//!
//! ```text
//! This is the API gateway <box id="gw" box="120 80 220 300" label="API gateway"/>
//! and every request goes through it <arrow from="gw" to="svc"/> to the service.
//! ```
//!
//! Tags may be split across arbitrary network chunks. Only *known* tag names
//! are treated as markup; anything else that looks like `<…>` (code, maths,
//! "a < b") is passed through as text, so a stray angle bracket can never
//! swallow speech.

use std::collections::BTreeMap;

/// Visual tags plus `task` (hand-off to the agent) and `lesson` (teaching mode).
pub const KNOWN_TAGS: &[&str] = &[
    "box", "circle", "highlight", "underline", "point", "arrow", "step", "spotlight", "zoom",
    "focus", "clear", "label", "task", "lesson", "board", "node", "sketch",
];

/// Longest tag we will buffer before deciding it is not a tag.
const MAX_TAG_LEN: usize = 600;

#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub name: String,
    pub attrs: BTreeMap<String, String>,
}

impl Tag {
    pub fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.get(k).map(|s| s.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Text(String),
    Tag(Tag),
}

pub struct MarkupParser {
    buf: String,
    tags: &'static [&'static str],
}

impl Default for MarkupParser {
    fn default() -> Self {
        Self { buf: String::new(), tags: KNOWN_TAGS }
    }
}

impl MarkupParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// A parser that recognises a different tag vocabulary (e.g. agent actions).
    pub fn with_tags(tags: &'static [&'static str]) -> Self {
        Self { buf: String::new(), tags }
    }

    /// Feed a chunk; returns every segment that is now unambiguous.
    pub fn push(&mut self, chunk: &str) -> Vec<Segment> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        loop {
            let Some(lt) = self.buf.find('<') else {
                emit_text(&mut out, std::mem::take(&mut self.buf));
                break;
            };
            if lt > 0 {
                let text: String = self.buf.drain(..lt).collect();
                emit_text(&mut out, text);
            }
            // buf now starts with '<'
            match classify(&self.buf, self.tags) {
                Candidate::NeedMore => break,
                Candidate::NotATag => {
                    let lt: String = self.buf.drain(..1).collect();
                    emit_text(&mut out, lt);
                }
                Candidate::Tag(tag, len) => {
                    self.buf.drain(..len);
                    out.push(Segment::Tag(tag));
                }
                Candidate::Drop(len) => {
                    // Unrecoverable markup: never speak it.
                    self.buf.drain(..len);
                }
            }
        }
        out
    }

    /// End of stream: whatever is buffered is text (an unterminated tag is
    /// never executed).
    pub fn finish(&mut self) -> Vec<Segment> {
        let mut out = Vec::new();
        let rest = std::mem::take(&mut self.buf);
        if let Some(lt) = rest.find('<') {
            // Drop an obviously truncated known tag rather than speaking it.
            if matches!(tag_name_prefix(&rest[lt..]), Some(n) if self.tags.contains(&n.to_ascii_lowercase().as_str())) {
                emit_text(&mut out, rest[..lt].to_string());
                return out;
            }
        }
        emit_text(&mut out, rest);
        out
    }
}

fn emit_text(out: &mut Vec<Segment>, s: String) {
    if s.is_empty() {
        return;
    }
    if let Some(Segment::Text(prev)) = out.last_mut() {
        prev.push_str(&s);
    } else {
        out.push(Segment::Text(s));
    }
}

enum Candidate {
    NeedMore,
    NotATag,
    Tag(Tag, usize),
    /// Clearly a known tag, but its attributes are beyond repair.
    Drop(usize),
}

fn tag_name_prefix(s: &str) -> Option<String> {
    let name: String = s[1..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

fn classify(s: &str, tags: &[&str]) -> Candidate {
    debug_assert!(s.starts_with('<'));
    let rest = &s[1..];
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if name.len() == rest.len() {
        // haven't seen the end of the name yet
        let could_be = rest.is_empty() || tags.iter().any(|t| t.starts_with(&name.to_ascii_lowercase()));
        return if could_be && s.len() < MAX_TAG_LEN { Candidate::NeedMore } else { Candidate::NotATag };
    }
    let lname = name.to_ascii_lowercase();
    if !tags.contains(&lname.as_str()) {
        return Candidate::NotATag;
    }
    let after = rest[name.len()..].chars().next().unwrap();
    if !(after.is_whitespace() || after == '/' || after == '>') {
        return Candidate::NotATag;
    }
    // Find the end: '>' outside quotes, or "/>" anywhere (models sometimes
    // drop a quote, and no label legitimately contains "/>").
    let mut quote: Option<char> = None;
    let mut prev = '\0';
    for (i, c) in s.char_indices().skip(1 + name.len()) {
        let self_close = c == '>' && prev == '/';
        prev = c;
        let end = match (quote, c) {
            _ if self_close => true,
            (Some(q), c) if c == q => {
                quote = None;
                false
            }
            (Some(_), _) => false,
            (None, '"') | (None, '\'') => {
                quote = Some(c);
                false
            }
            (None, '<') => return Candidate::NotATag,
            (None, '>') => true,
            _ => false,
        };
        if end {
            let inner = s[1 + name.len()..i].trim_end_matches('/');
            return match parse_attrs(inner).or_else(|| parse_attrs_lenient(inner)) {
                Some(attrs) => Candidate::Tag(Tag { name: lname, attrs }, i + 1),
                None => Candidate::Drop(i + 1),
            };
        }
        if i > MAX_TAG_LEN {
            return Candidate::NotATag;
        }
    }
    if s.len() > MAX_TAG_LEN {
        Candidate::NotATag
    } else {
        Candidate::NeedMore
    }
}

fn parse_attrs(s: &str) -> Option<BTreeMap<String, String>> {
    let mut attrs = BTreeMap::new();
    let mut chars = s.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            return Some(attrs);
        }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                key.push(c);
                chars.next();
            } else {
                break;
            }
        }
        if key.is_empty() {
            return None;
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek() != Some(&'=') {
            attrs.insert(key.to_ascii_lowercase(), String::new());
            continue;
        }
        chars.next();
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut val = String::new();
        match chars.peek().copied() {
            Some(q @ ('"' | '\'')) => {
                chars.next();
                loop {
                    match chars.next() {
                        Some(c) if c == q => break,
                        Some(c) => val.push(c),
                        None => return None,
                    }
                }
            }
            _ => {
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() {
                        break;
                    }
                    val.push(c);
                    chars.next();
                }
            }
        }
        attrs.insert(key.to_ascii_lowercase(), unescape(&val));
    }
}

const ATTR_KEYS: &[&str] = &[
    "id", "box", "label", "from", "to", "target", "n", "img", "text", "goal", "keys", "url", "dir", "amount",
    "risk", "button", "double", "clear", "ms", "question", "summary", "reason",
];

/// Best-effort recovery for malformed attribute lists such as
/// `box="[1,2,3,4] label="DB"` (missing quote). Each known key's value runs
/// until its closing quote or the next ` key=`, whichever comes first.
fn parse_attrs_lenient(s: &str) -> Option<BTreeMap<String, String>> {
    let lower = s.to_ascii_lowercase();
    let mut starts: Vec<(usize, usize, &str)> = Vec::new(); // (key_start, value_start, key)
    for key in ATTR_KEYS {
        let pat = format!("{key}=");
        let mut from = 0;
        while let Some(p) = lower[from..].find(&pat) {
            let at = from + p;
            let boundary = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric() && lower.as_bytes()[at - 1] != b'_';
            if boundary {
                starts.push((at, at + pat.len(), key));
                break;
            }
            from = at + pat.len();
        }
    }
    if starts.is_empty() {
        return None;
    }
    starts.sort();
    let mut attrs = BTreeMap::new();
    for (k, &(_, vstart, key)) in starts.iter().enumerate() {
        let vend = starts.get(k + 1).map(|n| n.0).unwrap_or(s.len());
        let mut v = s[vstart..vend].trim();
        let quote = v.chars().next().filter(|c| *c == '"' || *c == '\'');
        if let Some(q) = quote {
            v = &v[1..];
            if let Some(close) = v.find(q) {
                v = &v[..close];
            }
        }
        let v = v.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        if !v.is_empty() {
            attrs.insert(key.to_string(), unescape(v));
        }
    }
    (!attrs.is_empty()).then_some(attrs)
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&str]) -> Vec<Segment> {
        let mut p = MarkupParser::new();
        let mut out = Vec::new();
        for c in chunks {
            for s in p.push(c) {
                merge(&mut out, s);
            }
        }
        for s in p.finish() {
            merge(&mut out, s);
        }
        out
    }

    fn merge(out: &mut Vec<Segment>, s: Segment) {
        match (out.last_mut(), s) {
            (Some(Segment::Text(a)), Segment::Text(b)) => a.push_str(&b),
            (_, s) => out.push(s),
        }
    }

    fn text(s: &str) -> Segment {
        Segment::Text(s.into())
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(run(&["Hello ", "world."]), vec![text("Hello world.")]);
    }

    #[test]
    fn tag_in_one_chunk() {
        let out = run(&[r#"This <box id="a" box="1 2 3 4" label="DB"/> stores data."#]);
        assert_eq!(out.len(), 3);
        let Segment::Tag(t) = &out[1] else { panic!() };
        assert_eq!(t.name, "box");
        assert_eq!(t.attr("id"), Some("a"));
        assert_eq!(t.attr("box"), Some("1 2 3 4"));
        assert_eq!(t.attr("label"), Some("DB"));
        assert_eq!(out[2], text(" stores data."));
    }

    #[test]
    fn tag_split_at_every_byte() {
        let s = r#"Look <arrow from="a" to="b" label="x > y"/>here."#;
        for i in 1..s.len() {
            let out = run(&[&s[..i], &s[i..]]);
            assert_eq!(out.len(), 3, "split at {i}: {out:?}");
            assert_eq!(out[0], text("Look "));
            let Segment::Tag(t) = &out[1] else { panic!("split at {i}") };
            assert_eq!(t.attr("label"), Some("x > y"));
            assert_eq!(out[2], text("here."));
        }
    }

    #[test]
    fn non_tags_are_text() {
        assert_eq!(run(&["if a <b then", " x<3 and <div>"]), vec![text("if a <b then x<3 and <div>")]);
        assert_eq!(run(&["<", "boxer>"]), vec![text("<boxer>")]);
    }

    #[test]
    fn unterminated_known_tag_is_dropped_not_spoken() {
        assert_eq!(run(&[r#"Done <box id="a" box="1 2"#]), vec![text("Done ")]);
    }

    #[test]
    fn single_quotes_unquoted_and_case() {
        let out = run(&["<STEP n=2 target='db'/>"]);
        let Segment::Tag(t) = &out[0] else { panic!() };
        assert_eq!(t.name, "step");
        assert_eq!(t.attr("n"), Some("2"));
        assert_eq!(t.attr("target"), Some("db"));
    }

    #[test]
    fn non_self_closing_and_entities() {
        let out = run(&[r#"<clear>ok <label id="l" text="A &amp; B"/>"#]);
        assert!(matches!(&out[0], Segment::Tag(t) if t.name == "clear"));
        assert_eq!(out[1], text("ok "));
        assert!(matches!(&out[2], Segment::Tag(t) if t.attr("text") == Some("A & B")));
    }

    #[test]
    fn malformed_quotes_are_repaired_not_spoken() {
        let out = run(&[r#"That is the <box id="auth" box="[177,417,243,538] label="Auth service"/> Auth service."#]);
        assert_eq!(out.len(), 3, "{out:?}");
        let Segment::Tag(t) = &out[1] else { panic!("{out:?}") };
        assert_eq!(t.attr("box"), Some("[177,417,243,538]"));
        assert_eq!(t.attr("label"), Some("Auth service"));
        assert_eq!(out[2], text(" Auth service."));
    }

    #[test]
    fn unrepairable_known_tag_is_dropped() {
        let out = run(&[r#"Look <box ===/> here."#]);
        assert_eq!(out, vec![text("Look "), text(" here.")].into_iter().fold(Vec::new(), |mut v, s| {
            merge(&mut v, s);
            v
        }));
    }

    #[test]
    fn runaway_tag_falls_back_to_text() {
        let long = format!("<box label=\"{}", "x".repeat(700));
        let out = run(&[&long, "\"/> after"]);
        assert!(out.iter().all(|s| matches!(s, Segment::Text(_))));
    }
}
