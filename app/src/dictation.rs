use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

use crate::audio::Recorder;
use crate::overlay::{self, State};
use crate::settings::Settings;
use crate::{cleanup, echo, key, menu, models, output, transcribe, transcribe_stream};

enum Phase {
    Idle,
    Starting,
    Recording(Box<Recording>),
    Processing,
}

static PHASE: Mutex<Phase> = Mutex::new(Phase::Idle);

#[cfg(feature = "e2e")]
pub static FIXTURE: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
#[cfg(feature = "e2e")]
pub static MISSING_KEY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Recording {
    recorder: Option<Recorder>,
    model: String,
    key: String,
    stream: Option<transcribe_stream::Session>,
    stream_error: Option<String>,
    live: Arc<Mutex<String>>,
    started: Instant,
    echo: Arc<Mutex<echo::Status>>,
}

pub struct StartOptions {
    pub transcription_model: String,
    pub streaming: bool,
    pub remove_computer_audio: bool,
}

pub struct StopOptions {
    pub cleanup_model: Option<String>,
    pub instructions: Option<String>,
}

#[derive(Serialize)]
pub struct Outcome {
    pub raw: String,
    pub text: String,
    pub cleaned: bool,
    pub streamed: bool,
    pub error: Option<String>,
    pub timings: Value,
    pub echo: echo::Status,
}

impl StartOptions {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            streaming: models::find(&settings.transcription_model).is_some_and(|m| m.streaming()),
            transcription_model: settings.transcription_model.clone(),
            remove_computer_audio: settings.remove_computer_audio,
        }
    }
}

impl StopOptions {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            cleanup_model: settings.cleanup_model.clone(),
            instructions: Some(settings.instructions.trim().to_string()).filter(|s| !s.is_empty()),
        }
    }
}

pub enum Activity {
    Idle,
    Recording,
    Busy,
}

pub fn activity() -> Activity {
    match *PHASE.lock().unwrap() {
        Phase::Idle => Activity::Idle,
        Phase::Recording(_) => Activity::Recording,
        Phase::Starting | Phase::Processing => Activity::Busy,
    }
}

pub fn toggle() {
    let mut phase = PHASE.lock().unwrap();
    match std::mem::replace(&mut *phase, Phase::Processing) {
        Phase::Idle => {
            *phase = Phase::Starting;
            std::thread::spawn(|| {
                let settings = crate::settings::get();
                if let Err(error) = begin(StartOptions::from_settings(&settings)) {
                    overlay::show(State::Error(error));
                }
            });
        }
        Phase::Recording(recording) => {
            menu::show_recording(false);
            std::thread::spawn(move || {
                let settings = crate::settings::get();
                let outcome = finish(*recording, StopOptions::from_settings(&settings));
                deliver(&outcome, settings.output);
            });
        }
        busy => *phase = busy,
    }
}

fn deliver(outcome: &Outcome, output: crate::settings::Output) {
    if outcome.text.trim().is_empty() {
        return;
    }
    if output::deliver(&outcome.text, output) == output::Delivered::CopiedWithoutAccessibility {
        overlay::show(State::Error(
            "Copied. To paste, allow Accessibility from the Yapr menu.".into(),
        ));
    }
}

fn set_phase(phase: Phase) {
    let recording = matches!(phase, Phase::Recording(_));
    *PHASE.lock().unwrap() = phase;
    menu::show_recording(recording);
}

fn require_key() -> Result<String, String> {
    #[cfg(feature = "e2e")]
    if MISSING_KEY.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(key::MISSING.into());
    }
    key::require()
}

fn open_recorder(options: &StartOptions) -> Result<Option<Recorder>, String> {
    #[cfg(feature = "e2e")]
    if FIXTURE.get().is_some() {
        return Ok(None);
    }
    if options.remove_computer_audio {
        Recorder::start_with_echo()
    } else {
        Recorder::start()
    }
    .map(Some)
}

#[cfg(feature = "e2e")]
pub fn start(options: StartOptions) -> Result<(), String> {
    {
        let mut phase = PHASE.lock().unwrap();
        if !matches!(*phase, Phase::Idle) {
            return Err("Dictation is already running.".into());
        }
        *phase = Phase::Starting;
    }
    begin(options)
}

fn begin(options: StartOptions) -> Result<(), String> {
    let key = match require_key() {
        Ok(key) => key,
        Err(error) => {
            set_phase(Phase::Idle);
            return Err(error);
        }
    };
    let mut recorder = match open_recorder(&options) {
        Ok(recorder) => recorder,
        Err(error) => {
            set_phase(Phase::Idle);
            return Err(format!("Could not start the microphone: {error}"));
        }
    };
    let live = Arc::new(Mutex::new(String::new()));
    let echo = recorder.as_ref().map_or_else(
        || Arc::new(Mutex::new(echo::Status::default())),
        Recorder::echo_status,
    );
    let chunks = match recorder.as_mut() {
        Some(recorder) => recorder.take_chunks().unwrap(),
        None => fixture_chunks(),
    };
    let mut stream_error = None;
    let stream = if options.streaming {
        let preview = live.clone();
        match transcribe_stream::Session::start(
            options.transcription_model.clone(),
            key.clone(),
            chunks,
            move |_, text| *preview.lock().unwrap() = text.to_string(),
        ) {
            Ok(stream) => Some(stream),
            Err(error) => {
                stream_error = Some(error);
                None
            }
        }
    } else {
        drop(chunks);
        None
    };
    overlay::show(State::Listening("Listening 00:00".into()));
    set_phase(Phase::Recording(Box::new(Recording {
        recorder,
        model: options.transcription_model,
        key,
        stream,
        stream_error,
        live,
        started: Instant::now(),
        echo,
    })));
    std::thread::spawn(show_progress);
    Ok(())
}

#[cfg(feature = "e2e")]
fn fixture_chunks() -> std::sync::mpsc::Receiver<Vec<f32>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = tx.send(FIXTURE.get().cloned().unwrap_or_default());
    rx
}

#[cfg(not(feature = "e2e"))]
fn fixture_chunks() -> std::sync::mpsc::Receiver<Vec<f32>> {
    unreachable!("release recordings always use the microphone")
}

fn show_progress() {
    let mut notice_until = None;
    let mut notice_seen = false;
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let phase = PHASE.lock().unwrap();
        let Phase::Recording(recording) = &*phase else {
            return;
        };
        let reason = recording.echo.lock().unwrap().reason.clone();
        if reason.is_some() && !notice_seen {
            notice_seen = true;
            notice_until = Some(Instant::now() + Duration::from_secs(3));
        }
        if notice_until.is_some_and(|until| Instant::now() < until) {
            overlay::show(State::Error(reason.unwrap_or_default()));
            continue;
        }
        let live = recording.live.lock().unwrap().clone();
        let seconds = recording
            .started
            .elapsed()
            .as_secs()
            .min(crate::audio::MAX_SECONDS as u64);
        let text = if live.is_empty() {
            format!("Listening {:02}:{:02}", seconds / 60, seconds % 60)
        } else {
            live
        };
        overlay::show(State::Listening(text));
        if let Some(recorder) = &recording.recorder {
            overlay::level(recorder.level());
        }
    }
}

#[cfg(feature = "e2e")]
pub fn stop(options: StopOptions) -> Result<Outcome, String> {
    let recording = {
        let mut phase = PHASE.lock().unwrap();
        match std::mem::replace(&mut *phase, Phase::Processing) {
            Phase::Recording(recording) => recording,
            other => {
                *phase = other;
                return Err("Dictation is not running.".into());
            }
        }
    };
    menu::show_recording(false);
    Ok(finish(*recording, options))
}

fn finish(recording: Recording, options: StopOptions) -> Outcome {
    let outcome = process(recording, options);
    set_phase(Phase::Idle);
    outcome
}

fn process(recording: Recording, options: StopOptions) -> Outcome {
    let samples = match recording.recorder {
        Some(recorder) => recorder.finish(),
        None => recorded_fixture(),
    };
    let echo = recording.echo.lock().unwrap().clone();
    let started = Instant::now();
    let (streamed_text, stream_error) = match recording.stream {
        Some(stream) => match stream.finish() {
            Ok(text) => (Some(text), None),
            Err(error) => (None, Some(error)),
        },
        None => (None, recording.stream_error),
    };
    let streamed = streamed_text.is_some();
    let transcript = match streamed_text {
        Some(text) => Ok(text),
        None => {
            overlay::show(State::Transcribing);
            transcribe::transcribe(&samples, &recording.model, &recording.key).map_err(|error| {
                match stream_error.filter(|e| !e.is_empty()) {
                    Some(stream_error) => {
                        format!("Streaming failed: {stream_error} REST fallback failed: {error}")
                    }
                    None => error,
                }
            })
        }
    };
    let mut timings = json!({"transcribe_ms": started.elapsed().as_millis()});
    let raw = match transcript {
        Ok(raw) => raw,
        Err(error) => {
            overlay::show(State::Error(error.clone()));
            return Outcome {
                raw: String::new(),
                text: String::new(),
                cleaned: false,
                streamed: false,
                error: Some(error),
                timings,
                echo,
            };
        }
    };
    let mut outcome = Outcome {
        text: raw.clone(),
        raw,
        cleaned: false,
        streamed,
        error: None,
        timings: Value::Null,
        echo,
    };
    if outcome.raw.trim().is_empty() {
        outcome.text.clear();
        overlay::show(State::Done("No speech heard".into()));
    } else if let Some(model) = options.cleanup_model {
        overlay::show(State::Cleaning(String::new()));
        let started = Instant::now();
        let result = cleanup::cleanup(
            &outcome.raw,
            &model,
            options.instructions.as_deref(),
            &recording.key,
            |text| overlay::show(State::Cleaning(text.to_string())),
        );
        timings["cleanup_ms"] = json!(started.elapsed().as_millis());
        match result {
            Ok(result) => {
                outcome.text = result.text;
                outcome.cleaned = true;
                if let Some(ms) = result.first_text_ms {
                    timings["first_cleanup_text_ms"] = json!(ms);
                }
                overlay::show(State::Done(outcome.text.clone()));
            }
            Err(message) => {
                overlay::show(State::Error(message.clone()));
                outcome.error = Some(message);
            }
        }
    } else {
        overlay::show(State::Done(outcome.text.clone()));
    }
    outcome.timings = timings;
    outcome
}

#[cfg(feature = "e2e")]
fn recorded_fixture() -> Vec<f32> {
    FIXTURE.get().cloned().unwrap_or_default()
}

#[cfg(not(feature = "e2e"))]
fn recorded_fixture() -> Vec<f32> {
    unreachable!("release recordings always use the microphone")
}
