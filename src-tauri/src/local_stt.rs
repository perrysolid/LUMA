//! On-device speech-to-text, used only when the cloud recognizer fails
//! (no AssemblyAI key, credits used up, offline). Recognition runs on the
//! turn's buffered audio after the shortcut is released, so it adds a
//! second or so; nothing leaves the machine.
//!
//! macOS: Speech framework with `requiresOnDeviceRecognition`. Windows: not
//! available yet (Windows.Media.SpeechRecognition cannot take recorded
//! audio), so the cloud error is shown instead.

use anyhow::{anyhow, Result};

/// Transcribe 16 kHz mono PCM16 (little-endian bytes).
pub fn transcribe(pcm16le: &[u8], language: &str) -> Result<String> {
    if pcm16le.len() < 16_000 {
        return Ok(String::new()); // under half a second: nothing said
    }
    native::transcribe(pcm16le, language)
}

/// A 16-bit mono WAV file around raw PCM.
pub fn wav_bytes(pcm16le: &[u8], rate: u32) -> Vec<u8> {
    let mut w = Vec::with_capacity(44 + pcm16le.len());
    let len = pcm16le.len() as u32;
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&len.to_le_bytes());
    w.extend_from_slice(pcm16le);
    w
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::AllocAnyThread;
    use objc2_foundation::{NSError, NSLocale, NSString, NSURL};
    use objc2_speech::{
        SFSpeechRecognitionResult, SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus, SFSpeechURLRecognitionRequest,
    };
    use std::sync::{mpsc, Mutex};
    use std::time::Duration;

    /// Asking for permission without `NSSpeechRecognitionUsageDescription`
    /// in the app's Info.plist aborts the process, which is the case for an
    /// unbundled dev build. Only the installed app asks.
    fn bundled() -> bool {
        std::env::current_exe().is_ok_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
    }

    fn authorized() -> bool {
        let status = unsafe { SFSpeechRecognizer::authorizationStatus() };
        if status == SFSpeechRecognizerAuthorizationStatus::Authorized {
            return true;
        }
        if status != SFSpeechRecognizerAuthorizationStatus::NotDetermined || !bundled() {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(Some(tx));
        let block = RcBlock::new(move |s: SFSpeechRecognizerAuthorizationStatus| {
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(s);
            }
        });
        unsafe { SFSpeechRecognizer::requestAuthorization(&block) };
        rx.recv_timeout(Duration::from_secs(60)).is_ok_and(|s| s == SFSpeechRecognizerAuthorizationStatus::Authorized)
    }

    pub fn transcribe(pcm16le: &[u8], language: &str) -> Result<String> {
        if !authorized() {
            return Err(anyhow!(
                "on-device speech recognition isn't allowed (System Settings → Privacy & Security → Speech Recognition)"
            ));
        }
        // Speech reads audio from a file; it lives for the length of this
        // call only and is removed right after.
        let path = std::env::temp_dir().join(format!("luma-stt-{}-{}.wav", std::process::id(), rand_suffix()));
        std::fs::write(&path, wav_bytes(pcm16le, luma_net::STT_RATE))?;
        let r = recognize(&path, language);
        let _ = std::fs::remove_file(&path);
        r
    }

    fn rand_suffix() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
    }

    fn recognize(path: &std::path::Path, language: &str) -> Result<String> {
        let recognizer: Option<Retained<SFSpeechRecognizer>> = unsafe {
            let locale = NSLocale::initWithLocaleIdentifier(NSLocale::alloc(), &NSString::from_str(language));
            SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &locale)
                .or_else(|| SFSpeechRecognizer::init(SFSpeechRecognizer::alloc()))
        };
        let recognizer = recognizer.ok_or_else(|| anyhow!("no on-device recognizer for {language}"))?;
        if !unsafe { recognizer.supportsOnDeviceRecognition() } {
            return Err(anyhow!("on-device recognition isn't available for {language} on this Mac"));
        }
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let request = unsafe { SFSpeechURLRecognitionRequest::initWithURL(SFSpeechURLRecognitionRequest::alloc(), &url) };
        unsafe {
            request.setRequiresOnDeviceRecognition(true);
            request.setShouldReportPartialResults(false);
        }
        let (tx, rx) = mpsc::channel::<Result<String, String>>();
        let tx = Mutex::new(Some(tx));
        let handler = RcBlock::new(move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
            let msg = if let Some(e) = unsafe { error.as_ref() } {
                Some(Err(e.localizedDescription().to_string()))
            } else if let Some(r) = unsafe { result.as_ref() } {
                unsafe { r.isFinal() }.then(|| Ok(unsafe { r.bestTranscription().formattedString() }.to_string()))
            } else {
                None
            };
            if let Some(m) = msg {
                if let Some(tx) = tx.lock().unwrap().take() {
                    let _ = tx.send(m);
                }
            }
        });
        let task = unsafe { recognizer.recognitionTaskWithRequest_resultHandler(&request, &handler) };
        let out = rx.recv_timeout(Duration::from_secs(20));
        drop(task);
        match out {
            Ok(Ok(t)) => Ok(t),
            // "No speech detected" is an empty turn, not a failure.
            Ok(Err(e)) if e.to_lowercase().contains("no speech") => Ok(String::new()),
            Ok(Err(e)) => Err(anyhow!("on-device speech recognition failed: {e}")),
            Err(_) => Err(anyhow!("on-device speech recognition timed out")),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    pub fn transcribe(_pcm16le: &[u8], _language: &str) -> anyhow::Result<String> {
        Err(anyhow::anyhow!("on-device speech recognition is not available on this system yet"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wav_header_is_valid() {
        let w = super::wav_bytes(&[0u8; 3200], 16_000);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 36 + 3200);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 3200);
    }

    /// `cargo test -p luma local_stt -- --ignored --nocapture` (needs Speech
    /// Recognition permission for the process running the test).
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn recognizes_synthesized_speech() {
        let dir = std::env::temp_dir().join("luma-stt-probe");
        let _ = std::fs::create_dir_all(&dir);
        let wav = dir.join("probe.wav");
        let ok = std::process::Command::new("say")
            .args(["-o", wav.to_str().unwrap(), "--data-format=LEI16@16000", "Open the settings and turn off notifications"])
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "say failed");
        let bytes = std::fs::read(&wav).unwrap();
        let data = bytes.windows(4).position(|w| w == b"data").unwrap() + 8;
        let t = std::time::Instant::now();
        match super::transcribe(&bytes[data..], "en-US") {
            Ok(text) => println!("{} ms: {text:?}", t.elapsed().as_millis()),
            Err(e) => println!("not available here: {e}"),
        }
    }

    #[test]
    fn short_audio_is_an_empty_turn() {
        assert_eq!(super::transcribe(&[0u8; 100], "en-US").unwrap(), "");
    }
}
