//! Sound preview playback. The output device lives on its own thread (audio
//! handles are not `Send` on every platform); the UI sends it commands.

use std::io::Cursor;
use std::sync::mpsc::{Receiver, Sender, channel};

enum Command {
    Play(Vec<u8>),
    Stop,
}

pub struct Audio {
    tx: Option<Sender<Command>>,
}

impl Default for Audio {
    fn default() -> Self {
        Self::new()
    }
}

impl Audio {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let started = std::thread::Builder::new()
            .name("kombini-audio".into())
            .spawn(move || run(rx));
        if let Err(e) = &started {
            tracing::warn!(error = %e, "audio thread did not start");
        }
        Self {
            tx: started.ok().map(|_| tx),
        }
    }

    /// Plays a WAV or MP3 file, replacing whatever is playing.
    pub fn play(&self, bytes: Vec<u8>) {
        if let Some(tx) = &self.tx {
            tx.send(Command::Play(bytes)).ok();
        }
    }

    pub fn stop(&self) {
        if let Some(tx) = &self.tx {
            tx.send(Command::Stop).ok();
        }
    }
}

fn run(rx: Receiver<Command>) {
    // Opened on first use so an app without sound devices still starts.
    let mut device: Option<(rodio::MixerDeviceSink, rodio::Player)> = None;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::Stop => {
                if let Some((_, player)) = &device {
                    player.stop();
                }
            }
            Command::Play(bytes) => {
                if device.is_none() {
                    match rodio::DeviceSinkBuilder::open_default_sink() {
                        Ok(mut sink) => {
                            sink.log_on_drop(false);
                            let player = rodio::Player::connect_new(sink.mixer());
                            device = Some((sink, player));
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "no audio output device");
                            continue;
                        }
                    }
                }
                let Some((_, player)) = &device else { continue };
                player.stop();
                match rodio::Decoder::new(Cursor::new(bytes)) {
                    Ok(source) => {
                        player.append(source);
                        player.play();
                    }
                    Err(e) => tracing::warn!(error = %e, "sound could not be decoded"),
                }
            }
        }
    }
}
