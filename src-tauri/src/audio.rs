//! Microphone capture and speech playback.
//!
//! Both run on dedicated threads because the underlying audio streams are not
//! `Send` on every platform. Playback interleaves audio clips with callbacks,
//! so on-screen annotations fire exactly when their sentence starts.

use anyhow::{anyhow, Result};
use luma_core::audio::{i16_to_le_bytes, rms, MonoResampler};
use rodio::microphone::MicrophoneBuilder;
use rodio::Source;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc};
use tokio::sync::mpsc;

pub const STT_RATE: u32 = 16_000;
/// 100 ms of 16 kHz audio per websocket message (AssemblyAI accepts 50–1000 ms).
const CHUNK_SAMPLES: usize = 1_600;

pub struct MicHandle {
    stop: Arc<AtomicBool>,
}

impl MicHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for MicHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start recording. PCM16LE 16 kHz mono chunks go to `chunks`; the channel
/// closes when the handle is stopped. `level` receives a 0..1 meter value.
pub fn start_mic(chunks: mpsc::Sender<Vec<u8>>, level: impl Fn(f32) + Send + 'static) -> Result<MicHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let (ready_tx, ready_rx) = std_mpsc::channel::<Result<()>>();
    std::thread::Builder::new().name("luma-mic".into()).spawn(move || {
        let mic = match MicrophoneBuilder::new()
            .default_device()
            .and_then(|b| b.default_config())
            .map_err(|e| anyhow!("{e}"))
            .and_then(|b| b.open_stream().map_err(|e| anyhow!("{e}")))
        {
            Ok(m) => m,
            Err(e) => {
                let _ = ready_tx.send(Err(anyhow!("microphone unavailable (permission?): {e}")));
                return;
            }
        };
        let _ = ready_tx.send(Ok(()));
        let mut rs = MonoResampler::new(mic.sample_rate().get(), mic.channels().get(), STT_RATE);
        let mut pending: Vec<i16> = Vec::with_capacity(CHUNK_SAMPLES * 2);
        let mut block: Vec<f32> = Vec::with_capacity(4096);
        let mut mic = mic;
        while !stop2.load(Ordering::SeqCst) {
            block.clear();
            for _ in 0..1024 {
                match mic.next() {
                    Some(s) => block.push(s as f32),
                    None => break,
                }
            }
            if block.is_empty() {
                break;
            }
            pending.extend(rs.process(&block));
            while pending.len() >= CHUNK_SAMPLES {
                let chunk: Vec<i16> = pending.drain(..CHUNK_SAMPLES).collect();
                level(rms(&chunk));
                if chunks.blocking_send(i16_to_le_bytes(&chunk)).is_err() {
                    return;
                }
            }
        }
        if !pending.is_empty() {
            let _ = chunks.blocking_send(i16_to_le_bytes(&pending));
        }
    })?;
    ready_rx.recv().map_err(|_| anyhow!("microphone thread died"))??;
    Ok(MicHandle { stop })
}

pub enum SpeakerCmd {
    Play(Vec<u8>),
    Callback(Box<dyn Fn() + Send>),
    Stop,
}

/// Handle to the playback thread.
#[derive(Clone)]
pub struct Speaker {
    tx: std_mpsc::Sender<SpeakerCmd>,
}

impl Speaker {
    pub fn spawn() -> Result<Self> {
        let (tx, rx) = std_mpsc::channel::<SpeakerCmd>();
        let (ready_tx, ready_rx) = std_mpsc::channel::<Result<()>>();
        std::thread::Builder::new().name("luma-speaker".into()).spawn(move || {
            let mut sink = match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(anyhow!("no audio output: {e}")));
                    return;
                }
            };
            sink.log_on_drop(false);
            let _ = ready_tx.send(Ok(()));
            let mut player = rodio::Player::connect_new(sink.mixer());
            for cmd in rx {
                match cmd {
                    SpeakerCmd::Play(wav) => match rodio::Decoder::new(Cursor::new(wav)) {
                        Ok(d) => player.append(d),
                        Err(e) => log::warn!("undecodable audio clip: {e}"),
                    },
                    SpeakerCmd::Callback(f) => player.append(rodio::source::EmptyCallback::new(f)),
                    SpeakerCmd::Stop => {
                        // Dropping the queue (incl. pending callbacks) is the
                        // barge-in: nothing queued for the old turn may fire.
                        player.stop();
                        player = rodio::Player::connect_new(sink.mixer());
                    }
                }
            }
        })?;
        ready_rx.recv().map_err(|_| anyhow!("speaker thread died"))??;
        Ok(Self { tx })
    }

    pub fn send(&self, cmd: SpeakerCmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn stop(&self) {
        self.send(SpeakerCmd::Stop);
    }
}
