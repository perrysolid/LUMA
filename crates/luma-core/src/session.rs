//! Conversation memory for one companion session.
//!
//! Stores *text only*: what the user said, what LUMA said, and what LUMA
//! marked on screen (ids, labels, geometry). Screenshots and audio are never
//! retained after the turn that used them.

use crate::annotation::MarkedItem;
use crate::geometry::{view_to_norm, Display, SentImage};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Beginner,
    #[default]
    Standard,
    Technical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub user: String,
    pub assistant: String,
    /// "App — window title" when the turn happened.
    pub context_key: String,
}

#[derive(Debug, Clone)]
pub struct Session {
    turns: VecDeque<Turn>,
    items: HashMap<String, (MarkedItem, String)>,
    turn_counter: u32,
    pub level: Level,
    pub max_turns: usize,
}

impl Default for Session {
    fn default() -> Self {
        Self { turns: VecDeque::new(), items: HashMap::new(), turn_counter: 0, level: Level::default(), max_turns: 12 }
    }
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_turn_number(&self) -> u32 {
        self.turn_counter + 1
    }

    pub fn turns(&self) -> impl Iterator<Item = &Turn> {
        self.turns.iter()
    }

    pub fn last_turn(&self) -> Option<&Turn> {
        self.turns.back()
    }

    /// Items marked while looking at the same app/window, most recent last.
    pub fn items_for(&self, context_key: &str) -> Vec<MarkedItem> {
        let mut v: Vec<MarkedItem> = self
            .items
            .values()
            .filter(|(_, k)| k == context_key)
            .map(|(m, _)| m.clone())
            .collect();
        v.sort_by_key(|m| (m.turn, m.id.clone()));
        v
    }

    pub fn record(&mut self, turn: Turn, items: impl IntoIterator<Item = MarkedItem>) {
        self.turn_counter += 1;
        let key = turn.context_key.clone();
        for m in items {
            self.items.insert(m.id.clone(), (m, key.clone()));
        }
        // keep the item map bounded: drop items older than the history window
        let horizon = self.turn_counter.saturating_sub(self.max_turns as u32);
        self.items.retain(|_, (m, _)| m.turn > horizon);
        self.turns.push_back(turn);
        while self.turns.len() > self.max_turns {
            self.turns.pop_front();
        }
    }

    /// Updates the remembered explanation level from phrases like
    /// "explain like I'm a beginner" so it persists across follow-ups.
    pub fn observe_level_request(&mut self, user: &str) -> Option<Level> {
        let u = user.to_lowercase();
        let lvl = if ["beginner", "like i'm five", "like i am five", "eli5", "simple terms", "simpler", "new to this"]
            .iter()
            .any(|p| u.contains(p))
        {
            Level::Beginner
        } else if ["technical", "in depth", "in-depth", "expert", "under the hood", "deeper", "advanced"]
            .iter()
            .any(|p| u.contains(p))
        {
            Level::Technical
        } else if u.contains("normal level") || u.contains("regular explanation") {
            Level::Standard
        } else {
            return None;
        };
        self.level = lvl;
        Some(lvl)
    }

    /// Privacy: forget everything.
    pub fn clear(&mut self) {
        *self = Self { level: self.level, max_turns: self.max_turns, ..Self::default() };
    }

    /// Describe previously marked items in the coordinates of the image the
    /// model is about to see, so "the one you just explained" can be resolved
    /// and the model can reuse ids with `target=`.
    pub fn describe_items(&self, context_key: &str, img: &SentImage, displays: &[Display]) -> String {
        let mut lines = Vec::new();
        for m in self.items_for(context_key) {
            let Some(d) = displays.iter().find(|d| d.index == m.at.display_index) else { continue };
            let label = m.label.as_deref().unwrap_or("(unlabelled)");
            let pos = if d.index == img.capture.display_index {
                view_to_norm(&m.at.rect, img, d)
                    .map(|b| format!("box=\"{:.0} {:.0} {:.0} {:.0}\"", b.ymin, b.xmin, b.ymax, b.xmax))
                    .unwrap_or_else(|| "off-image".into())
            } else {
                format!("on another display ({})", d.name)
            };
            lines.push(format!("- id=\"{}\" label=\"{}\" {} (turn {})", m.id, label, pos, m.turn));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Capture, DisplayRect, Rect};

    fn item(id: &str, turn: u32) -> MarkedItem {
        MarkedItem {
            id: id.into(),
            label: Some(id.to_uppercase()),
            at: DisplayRect { display_index: 0, rect: Rect::new(100.0, 50.0, 100.0, 50.0) },
            turn,
        }
    }

    fn turn(key: &str) -> Turn {
        Turn { user: "q".into(), assistant: "a".into(), context_key: key.into() }
    }

    #[test]
    fn items_are_scoped_to_the_window_they_were_marked_in() {
        let mut s = Session::new();
        s.record(turn("PowerPoint — deck"), [item("db", 1)]);
        s.record(turn("Chrome — docs"), [item("nav", 2)]);
        assert_eq!(s.items_for("PowerPoint — deck").len(), 1);
        assert_eq!(s.items_for("Chrome — docs")[0].id, "nav");
    }

    #[test]
    fn history_and_items_are_bounded() {
        let mut s = Session { max_turns: 3, ..Session::new() };
        for i in 1..=10 {
            s.record(turn("k"), [item(&format!("i{i}"), i)]);
        }
        assert_eq!(s.turns().count(), 3);
        let ids: Vec<_> = s.items_for("k").into_iter().map(|m| m.id).collect();
        assert_eq!(ids, vec!["i8", "i9", "i10"]);
    }

    #[test]
    fn level_requests_persist_and_clear_keeps_preference() {
        let mut s = Session::new();
        assert_eq!(s.observe_level_request("Explain this like I'm a beginner"), Some(Level::Beginner));
        assert_eq!(s.observe_level_request("what is this?"), None);
        assert_eq!(s.level, Level::Beginner);
        assert_eq!(s.observe_level_request("give me the technical version"), Some(Level::Technical));
        s.record(turn("k"), [item("a", 1)]);
        s.clear();
        assert_eq!(s.turns().count(), 0);
        assert!(s.items_for("k").is_empty());
        assert_eq!(s.level, Level::Technical);
    }

    #[test]
    fn describe_items_uses_current_image_coordinates() {
        let mut s = Session::new();
        s.record(turn("k"), [item("db", 1)]);
        let d = Display {
            index: 0,
            name: "d".into(),
            input_frame: Rect::new(0.0, 0.0, 1000.0, 500.0),
            input_per_point: 1.0,
            scale_factor: 2.0,
            is_primary: true,
        };
        let img = SentImage::full(Capture { display_index: 0, width_px: 2000, height_px: 1000 }, 1000);
        let text = s.describe_items("k", &img, &[d]);
        assert_eq!(text, "- id=\"db\" label=\"DB\" box=\"100 100 200 200\" (turn 1)");
    }
}
