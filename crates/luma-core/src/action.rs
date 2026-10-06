//! Agent actions: parsing the model's action tags, and the safety policy that
//! decides which actions need the user's explicit approval.
//!
//! The model proposes; LUMA disposes. Every action is validated here and the
//! approval policy is enforced by code, not by trusting the model's own
//! `risk` attribute (it can only make things *stricter*).

use crate::geometry::{norm_to_view, Display, DisplayRect, NormBox, SentImage};
use crate::markup::Tag;
use serde::{Deserialize, Serialize};

pub const AGENT_TAGS: &[&str] = &["click", "type", "key", "scroll", "open", "wait", "ask", "done", "fail"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Action {
    Click { at: DisplayRect, button: MouseButton, double: bool, label: String },
    Type { text: String, clear_first: bool },
    Key { keys: Vec<String> },
    Scroll { at: Option<DisplayRect>, down: bool, amount: i32 },
    Open { url: String },
    Wait { ms: u64 },
    Ask { question: String },
    Done { summary: String },
    Fail { reason: String },
}

impl Action {
    /// Short human description for the action log ("what did you change?").
    pub fn describe(&self) -> String {
        match self {
            Action::Click { label, button, double, .. } => {
                let verb = match (button, double) {
                    (MouseButton::Right, _) => "right-clicked",
                    (_, true) => "double-clicked",
                    _ => "clicked",
                };
                format!("{verb} \"{label}\"")
            }
            Action::Type { text, .. } => format!("typed \"{}\"", truncate(text, 60)),
            Action::Key { keys } => format!("pressed {}", keys.join("+")),
            Action::Scroll { down, amount, .. } => format!("scrolled {} {amount}", if *down { "down" } else { "up" }),
            Action::Open { url } => format!("opened {url}"),
            Action::Wait { ms } => format!("waited {ms} ms"),
            Action::Ask { question } => format!("asked: {question}"),
            Action::Done { summary } => format!("finished: {summary}"),
            Action::Fail { reason } => format!("stopped: {reason}"),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Action::Done { .. } | Action::Fail { .. })
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedAction {
    pub action: Action,
    /// The model flagged this step as consequential.
    pub model_risk_high: bool,
}

/// Turn an action tag into a validated action. Geometry is on image 1.
pub fn parse_action(tag: &Tag, img: &SentImage, display: &Display) -> Result<ParsedAction, String> {
    let model_risk_high = tag.attr("risk").is_some_and(|r| r.eq_ignore_ascii_case("high"));
    let rect = |key: &str| -> Option<DisplayRect> {
        let raw = tag.attr(key)?;
        let n: Vec<f64> = raw
            .split(|c: char| c.is_whitespace() || c == ',' || c == '[' || c == ']')
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.parse().ok())
            .collect();
        let b = match n.len() {
            4 => NormBox::sanitized([n[0], n[1], n[2], n[3]]),
            2 => NormBox::from_point(n[0], n[1]),
            _ => None,
        }?;
        Some(norm_to_view(&b, img, display))
    };
    let text = |k: &str| tag.attr(k).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let action = match tag.name.as_str() {
        "click" => Action::Click {
            at: rect("box").ok_or("click needs box")?,
            button: if tag.attr("button").is_some_and(|b| b.eq_ignore_ascii_case("right")) {
                MouseButton::Right
            } else {
                MouseButton::Left
            },
            double: tag.attr("double").is_some_and(|d| d == "true"),
            label: text("label").unwrap_or_else(|| "that".into()),
        },
        "type" => Action::Type {
            text: tag.attr("text").filter(|t| !t.is_empty()).ok_or("type needs text")?.to_string(),
            clear_first: tag.attr("clear").is_some_and(|d| d == "true"),
        },
        "key" => {
            let keys = parse_keys(tag.attr("keys").ok_or("key needs keys")?)?;
            Action::Key { keys }
        }
        "scroll" => Action::Scroll {
            at: rect("box"),
            down: !tag.attr("dir").is_some_and(|d| d.eq_ignore_ascii_case("up")),
            amount: tag.attr("amount").and_then(|a| a.trim().parse().ok()).unwrap_or(5).clamp(1, 30),
        },
        "open" => {
            let url = text("url").ok_or("open needs url")?;
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err(format!("refusing to open non-web URL {url:?}"));
            }
            Action::Open { url }
        }
        "wait" => Action::Wait { ms: tag.attr("ms").and_then(|m| m.trim().parse().ok()).unwrap_or(1000).clamp(100, 5000) },
        "ask" => Action::Ask { question: text("question").or_else(|| text("text")).ok_or("ask needs question")? },
        "done" => Action::Done { summary: text("summary").unwrap_or_else(|| "Done.".into()) },
        "fail" => Action::Fail { reason: text("reason").unwrap_or_else(|| "I couldn't finish that.".into()) },
        other => return Err(format!("unknown action {other}")),
    };
    Ok(ParsedAction { action, model_risk_high })
}

pub const MODIFIERS: &[&str] = &["cmd", "ctrl", "alt", "shift"];
pub const NAMED_KEYS: &[&str] = &[
    "enter", "tab", "escape", "space", "backspace", "delete", "up", "down", "left", "right", "home", "end",
    "pageup", "pagedown",
];

/// "Cmd+Shift+L" → ["cmd","shift","l"]. Validates every part.
pub fn parse_keys(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for raw in s.split('+').map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()) {
        let k = match raw.as_str() {
            "command" | "meta" | "super" | "win" | "⌘" => "cmd".to_string(),
            "control" | "ctl" => "ctrl".to_string(),
            "option" | "opt" | "alt" | "⌥" => "alt".to_string(),
            "return" => "enter".to_string(),
            "esc" => "escape".to_string(),
            "arrowup" => "up".to_string(),
            "arrowdown" => "down".to_string(),
            "arrowleft" => "left".to_string(),
            "arrowright" => "right".to_string(),
            k => k.to_string(),
        };
        let valid = MODIFIERS.contains(&k.as_str())
            || NAMED_KEYS.contains(&k.as_str())
            || (k.chars().count() == 1)
            || (k.starts_with('f') && k[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)));
        if !valid {
            return Err(format!("unknown key {raw:?}"));
        }
        out.push(k);
    }
    if out.is_empty() || out.iter().all(|k| MODIFIERS.contains(&k.as_str())) {
        return Err("key combo needs a non-modifier key".into());
    }
    Ok(out)
}

// ---------------------------------------------------------------- policy

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    /// Looking, scrolling, opening a page, focusing a field.
    Safe,
    /// Changes something the user could lose, publish, pay for or be locked out by.
    NeedsApproval,
    /// Never done by LUMA.
    Forbidden,
}

/// Words on a button that mean "this commits something". Opening a dialog
/// ("Change username") is fine; the committing step ("Change my username",
/// "Delete", "Send") needs approval. The model's `risk="high"` covers the rest
/// (e.g. a generic "Save" on an account page).
const CONSEQUENTIAL_WORDS: &[&str] = &[
    "delete", "remove", "erase", "discard", "destroy", "send", "submit", "post", "publish", "share", "tweet",
    "pay", "purchase", "buy", "place order", "checkout", "subscribe", "unsubscribe", "transfer", "withdraw",
    "deposit", "deactivate", "close account", "sign out", "log out", "logout", "uninstall", "reset", "revoke",
    "merge", "deploy", "confirm", "change my", "transfer ownership", "permanently",
];

const SECRET_WORDS: &[&str] = &["password", "passcode", "2fa", "otp", "one-time code", "secret", "api key", "token", "cvv", "card number"];

/// Decide whether an action may run without asking. `context` is any text
/// that describes what the action touches (labels, nearby field name).
pub fn assess(action: &Action, model_risk_high: bool, context: &str) -> Risk {
    let ctx = context.to_lowercase();
    match action {
        Action::Type { text, .. } => {
            if SECRET_WORDS.iter().any(|w| ctx.contains(w)) || looks_like_secret(text) {
                return Risk::Forbidden;
            }
            if model_risk_high {
                Risk::NeedsApproval
            } else {
                Risk::Safe
            }
        }
        Action::Click { label, .. } => {
            let l = label.to_lowercase();
            if model_risk_high || CONSEQUENTIAL_WORDS.iter().any(|w| contains_word(&l, w)) {
                Risk::NeedsApproval
            } else {
                Risk::Safe
            }
        }
        Action::Key { keys } => {
            // Enter can submit a form; destructive shortcuts need approval.
            let destructive = keys.iter().any(|k| k == "delete" || k == "backspace") && keys.iter().any(|k| k == "cmd");
            if model_risk_high || destructive || (keys == &["cmd", "q"]) {
                Risk::NeedsApproval
            } else {
                Risk::Safe
            }
        }
        Action::Open { .. } | Action::Scroll { .. } | Action::Wait { .. } => {
            if model_risk_high {
                Risk::NeedsApproval
            } else {
                Risk::Safe
            }
        }
        Action::Ask { .. } | Action::Done { .. } | Action::Fail { .. } => Risk::Safe,
    }
}

fn contains_word(hay: &str, needle: &str) -> bool {
    hay.match_indices(needle).any(|(i, _)| {
        let before = hay[..i].chars().last().is_none_or(|c| !c.is_alphanumeric());
        let after = hay[i + needle.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        before && after
    })
}

/// Heuristic: long random-looking tokens are probably secrets.
fn looks_like_secret(t: &str) -> bool {
    let t = t.trim();
    if t.contains(' ') || t.len() < 20 {
        return false;
    }
    let digits = t.chars().filter(|c| c.is_ascii_digit()).count();
    let upper = t.chars().filter(|c| c.is_ascii_uppercase()).count();
    let lower = t.chars().filter(|c| c.is_ascii_lowercase()).count();
    digits > 2 && upper > 2 && lower > 2
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YesNo {
    Yes,
    No,
    Unclear,
}

/// Interpret a spoken reply to "should I go ahead?".
pub fn yes_no(reply: &str) -> YesNo {
    let r = reply.to_lowercase();
    let words: Vec<&str> = r.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).collect();
    let has = |p: &str| {
        if p.contains(' ') {
            r.contains(p)
        } else {
            words.contains(&p)
        }
    };
    let no = ["no", "nope", "don't", "dont", "stop", "cancel", "wait", "abort", "not", "never", "nah"].iter().any(|p| has(p));
    let yes = ["yes", "yeah", "yep", "yup", "sure", "ok", "okay", "go ahead", "do it", "confirm", "proceed", "continue", "please do", "correct", "haan", "ha"]
        .iter()
        .any(|p| has(p));
    match (yes, no) {
        (true, false) => YesNo::Yes,
        (false, true) => YesNo::No,
        _ => YesNo::Unclear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Capture, Rect};
    use crate::markup::{MarkupParser, Segment};

    fn setup() -> (Display, SentImage) {
        let d = Display {
            index: 0,
            name: "d".into(),
            input_frame: Rect::new(0.0, 0.0, 1000.0, 500.0),
            input_per_point: 1.0,
            scale_factor: 2.0,
            is_primary: true,
        };
        (d, SentImage::full(Capture { display_index: 0, width_px: 2000, height_px: 1000 }, 1000))
    }

    fn tag(s: &str) -> Tag {
        let mut p = MarkupParser::with_tags(AGENT_TAGS);
        p.push(s)
            .into_iter()
            .chain(p.finish())
            .find_map(|s| match s {
                Segment::Tag(t) => Some(t),
                _ => None,
            })
            .expect("a tag")
    }

    #[test]
    fn parses_every_action() {
        let (d, img) = setup();
        let p = |s: &str| parse_action(&tag(s), &img, &d).unwrap().action;
        match p(r#"<click box="100 100 200 200" label="Settings"/>"#) {
            Action::Click { at, label, button, double } => {
                assert_eq!(at.rect, Rect::new(100.0, 50.0, 100.0, 50.0));
                assert_eq!((label.as_str(), button, double), ("Settings", MouseButton::Left, false));
            }
            o => panic!("{o:?}"),
        }
        assert!(matches!(p(r#"<click box="500 500" button="right" double="true"/>"#), Action::Click { button: MouseButton::Right, double: true, .. }));
        assert_eq!(p(r#"<type text="perry-solid" clear="true"/>"#), Action::Type { text: "perry-solid".into(), clear_first: true });
        assert_eq!(p(r#"<key keys="Command+L"/>"#), Action::Key { keys: vec!["cmd".into(), "l".into()] });
        assert_eq!(p(r#"<scroll dir="up" amount="3"/>"#), Action::Scroll { at: None, down: false, amount: 3 });
        assert_eq!(p(r#"<open url="https://github.com/settings/admin"/>"#), Action::Open { url: "https://github.com/settings/admin".into() });
        assert_eq!(p(r#"<wait ms="99999"/>"#), Action::Wait { ms: 5000 });
        assert_eq!(p(r#"<ask question="What new username?"/>"#), Action::Ask { question: "What new username?".into() });
        assert!(p(r#"<done summary="Renamed."/>"#).is_terminal());
    }

    #[test]
    fn rejects_bad_actions() {
        let (d, img) = setup();
        for s in [
            r#"<click label="x"/>"#,
            r#"<type text=""/>"#,
            r#"<key keys="cmd"/>"#,
            r#"<key keys="hyper+x"/>"#,
            r#"<open url="file:///etc/passwd"/>"#,
            r#"<open url="javascript:alert(1)"/>"#,
        ] {
            assert!(parse_action(&tag(s), &img, &d).is_err(), "{s}");
        }
    }

    #[test]
    fn policy_requires_approval_for_consequential_clicks() {
        let click = |label: &str| Action::Click {
            at: DisplayRect { display_index: 0, rect: Rect::new(0.0, 0.0, 1.0, 1.0) },
            button: MouseButton::Left,
            double: false,
            label: label.into(),
        };
        assert_eq!(assess(&click("Settings"), false, ""), Risk::Safe);
        assert_eq!(assess(&click("Account"), false, ""), Risk::Safe);
        assert_eq!(assess(&click("Change username"), false, ""), Risk::Safe, "only opens a dialog");
        assert_eq!(assess(&click("Change my username"), false, ""), Risk::NeedsApproval);
        assert_eq!(assess(&click("Delete repository"), false, ""), Risk::NeedsApproval);
        assert_eq!(assess(&click("Send"), false, ""), Risk::NeedsApproval);
        assert_eq!(assess(&click("Sender details"), false, ""), Risk::Safe, "whole words only");
        assert_eq!(assess(&click("Next"), true, ""), Risk::NeedsApproval, "model can only make it stricter");
    }

    #[test]
    fn policy_never_types_secrets() {
        let t = |s: &str| Action::Type { text: s.into(), clear_first: false };
        assert_eq!(assess(&t("perry-solid"), false, "username field"), Risk::Safe);
        assert_eq!(assess(&t("hunter2"), false, "Password"), Risk::Forbidden);
        assert_eq!(assess(&t("ghp_A1b2C3d4E5f6G7h8I9j0KLmn"), false, ""), Risk::Forbidden);
        assert_eq!(assess(&Action::Key { keys: vec!["cmd".into(), "backspace".into()] }, false, ""), Risk::NeedsApproval);
        assert_eq!(assess(&Action::Key { keys: vec!["cmd".into(), "l".into()] }, false, ""), Risk::Safe);
    }

    #[test]
    fn spoken_yes_no() {
        assert_eq!(yes_no("Yes, go ahead."), YesNo::Yes);
        assert_eq!(yes_no("yeah do it"), YesNo::Yes);
        assert_eq!(yes_no("Okay."), YesNo::Yes);
        assert_eq!(yes_no("No, stop."), YesNo::No);
        assert_eq!(yes_no("Don't do that"), YesNo::No);
        assert_eq!(yes_no("What does it do?"), YesNo::Unclear);
        assert_eq!(yes_no("yes no"), YesNo::Unclear);
    }

    #[test]
    fn describe_for_action_log() {
        assert_eq!(Action::Type { text: "abc".into(), clear_first: false }.describe(), "typed \"abc\"");
        assert_eq!(Action::Key { keys: vec!["cmd".into(), "z".into()] }.describe(), "pressed cmd+z");
    }
}
