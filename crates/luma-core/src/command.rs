//! Short spoken commands handled locally, without a model call.
//!
//! Only whole utterances match ("repeat that", "never mind"), never a
//! command word inside a real question ("how do I repeat a cell in Excel?").

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalCommand {
    /// Stop and do nothing more.
    Cancel,
    /// Play the last answer again, with its drawings.
    Repeat,
    /// "What did you change?": read back the last task's actions.
    WhatChanged,
    /// "Undo that": undo the last task's change where the app allows it.
    Undo,
}

/// Politeness and address words that do not change the command.
const FILLER: &[&str] = &[
    "please", "pls", "luma", "hey", "ok", "okay", "um", "uh", "hmm", "sorry", "can", "could", "would", "you", "just",
    "oh", "so", "actually", "ji",
];

const CANCEL: &[&str] = &[
    "stop", "cancel", "never mind", "nevermind", "forget it", "forget about it", "that's all", "thats all",
    "that's it", "thats it", "nothing", "no thanks", "no thank you", "rehne do", "chhodo", "bas",
    "stop the lesson", "end the lesson", "quit the lesson", "stop teaching", "stop teaching me", "i'm done", "im done",
];

const REPEAT: &[&str] = &[
    "repeat", "repeat that", "repeat it", "repeat that again", "say that again", "say it again", "come again",
    "what did you say", "what was that", "pardon", "pardon me", "one more time", "once more", "again",
    "show me again", "show that again", "phir se", "phir se bolo", "dobara", "dobara bolo", "ek baar aur",
];

const WHAT_CHANGED: &[&str] = &[
    "what did you change", "what did you do", "what have you done", "what did you just do", "what did you change exactly",
    "what changed", "what have you changed", "tell me what you changed", "what all did you change", "kya badla",
];

const UNDO: &[&str] = &[
    "undo", "undo that", "undo it", "undo this", "undo what you did", "undo your change", "undo the change", "revert that",
    "revert it", "take that back", "put it back", "change it back", "wapas karo",
];

pub fn local_command(transcript: &str) -> Option<LocalCommand> {
    let lower = transcript.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty() && !FILLER.contains(w))
        .collect();
    if words.is_empty() || words.len() > 5 {
        return None;
    }
    let phrase = words.join(" ");
    // The lists are written naturally; strip the same filler from them.
    let matches = |list: &[&str]| {
        list.iter().any(|p| {
            p.split(' ').filter(|w| !FILLER.contains(w)).collect::<Vec<_>>().join(" ") == phrase
        })
    };
    if matches(CANCEL) {
        Some(LocalCommand::Cancel)
    } else if matches(REPEAT) {
        Some(LocalCommand::Repeat)
    } else if matches(WHAT_CHANGED) {
        Some(LocalCommand::WhatChanged)
    } else if matches(UNDO) {
        Some(LocalCommand::Undo)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_utterance_commands() {
        assert_eq!(local_command("Never mind."), Some(LocalCommand::Cancel));
        assert_eq!(local_command("Okay, stop!"), Some(LocalCommand::Cancel));
        assert_eq!(local_command("Sorry, can you say that again?"), Some(LocalCommand::Repeat));
        assert_eq!(local_command("Luma, repeat that please"), Some(LocalCommand::Repeat));
        assert_eq!(local_command("phir se bolo"), Some(LocalCommand::Repeat));
        assert_eq!(local_command("What did you change?"), Some(LocalCommand::WhatChanged));
        assert_eq!(local_command("Okay, undo that please."), Some(LocalCommand::Undo));
        assert_eq!(local_command("Stop the lesson."), Some(LocalCommand::Cancel));
    }

    #[test]
    fn real_questions_are_not_commands() {
        assert_eq!(local_command("How do I repeat a row in Excel?"), None);
        assert_eq!(local_command("Why did the build stop?"), None);
        assert_eq!(local_command("What does cancel subscription do"), None);
        assert_eq!(local_command(""), None);
        assert_eq!(local_command("okay"), None);
        assert_eq!(local_command("How do I undo a commit in git?"), None);
        assert_eq!(local_command("what did you change in the formula and why"), None);
    }
}
