mod audio;
mod audio_timing;
mod cleanup;
mod dictation;
mod echo;
#[cfg(feature = "e2e")]
mod echo_test;
mod gateway;
mod hotkey;
mod key;
mod menu;
mod models;
mod output;
mod overlay;
mod permissions;
mod settings;
mod system_audio;
mod transcribe;
mod transcribe_stream;

use std::process::ExitCode;

use overlay::State;

#[cfg(not(feature = "e2e"))]
const USAGE: &str = "usage: Yapr (run the menu bar app)";
#[cfg(feature = "e2e")]
const USAGE: &str = "usage:
  Yapr                                          run the menu bar app
  Yapr --fixture <wav>                          menu bar app, dictating the WAV instead of the mic
  Yapr --dictate <wav|mic> --model <id> --cleanup <id|none> [--streaming] [--instructions <text>]
       [--keep-computer-audio] [--missing-key] [--deny-system-audio] [--seconds <n>]
  Yapr --list-models
  Yapr --transcribe <wav> --model <id>
  Yapr --stream <wav> --model <id>
  Yapr --cleanup <text> --model <id>
  Yapr --echo-test <music.wav|none> <speech.wav> --model <id>
  Yapr --echo-offline <mic.wav> <reference.wav> <target.wav>
  Yapr --test-capture                           mic + system audio, no gateway calls
  Yapr --check-key-leaks <path>...";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => run_menu_bar(),
        #[cfg(feature = "e2e")]
        ["--fixture", path] => audio::load_wav(std::path::Path::new(path)).and_then(|samples| {
            let _ = dictation::FIXTURE.set(samples);
            run_menu_bar()
        }),
        #[cfg(feature = "e2e")]
        ["--dictate", rest @ ..] => {
            let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
            run_app(move || dictate(&rest))
        }
        #[cfg(feature = "e2e")]
        ["--list-models"] => list_models(),
        #[cfg(feature = "e2e")]
        ["--test-capture"] => run_app(test_capture),
        #[cfg(feature = "e2e")]
        ["--transcribe", path, "--model", model] => transcribe_file(path, model),
        #[cfg(feature = "e2e")]
        ["--stream", path, "--model", model] => stream_file(path, model),
        #[cfg(feature = "e2e")]
        ["--cleanup", text, "--model", model] => clean_text(text, model),
        #[cfg(feature = "e2e")]
        ["--echo-test", music, speech, "--model", model] => {
            echo_test::run(music, speech, model).map_err(|e| e.to_string())
        }
        #[cfg(feature = "e2e")]
        ["--echo-offline", mic, reference, target] => {
            echo_test::offline(mic, reference, target).map_err(|e| e.to_string())
        }
        #[cfg(feature = "e2e")]
        ["--check-key-leaks", paths @ ..] if !paths.is_empty() => check_key_leaks(paths),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run_menu_bar() -> Result<(), String> {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    let mtm = objc2::MainThreadMarker::new().ok_or("must start on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    menu::install(mtm);
    std::thread::spawn(|| {
        if let Err(error) = models::refresh() {
            eprintln!("model list: {error}");
        }
    });
    std::thread::spawn(|| {
        if matches!(key::load(), Ok(None)) {
            overlay::show(State::Error(
                "Finish setup from the Yapr icon in the menu bar.".into(),
            ));
        }
    });
    app.run();
    Ok(())
}

#[cfg(feature = "e2e")]
fn run_app(work: impl FnOnce() -> Result<(), String> + Send + 'static) -> Result<(), String> {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    let mtm = objc2::MainThreadMarker::new().ok_or("must start on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    std::thread::spawn(move || {
        let code = match work() {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("{error}");
                1
            }
        };
        std::process::exit(code);
    });
    app.run();
    Ok(())
}

#[cfg(feature = "e2e")]
fn dictate(args: &[String]) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let mut args = args.iter().map(String::as_str);
    let source = args.next().ok_or(USAGE)?;
    let mut start = dictation::StartOptions {
        transcription_model: String::new(),
        streaming: false,
        remove_computer_audio: true,
    };
    let mut stop = dictation::StopOptions {
        cleanup_model: None,
        instructions: None,
    };
    let mut seconds = 1.5;
    while let Some(flag) = args.next() {
        let mut value = || args.next().map(str::to_string).ok_or(USAGE);
        match flag {
            "--model" => start.transcription_model = value()?,
            "--cleanup" => stop.cleanup_model = Some(value()?).filter(|m| m != "none"),
            "--instructions" => stop.instructions = Some(value()?),
            "--seconds" => seconds = value()?.parse().map_err(|_| USAGE)?,
            "--streaming" => start.streaming = true,
            "--keep-computer-audio" => start.remove_computer_audio = false,
            "--missing-key" => dictation::MISSING_KEY.store(true, Ordering::Relaxed),
            "--deny-system-audio" => system_audio::simulate_permission_denial(),
            _ => return Err(USAGE.into()),
        }
    }
    if start.transcription_model.is_empty() {
        return Err(USAGE.into());
    }
    if source != "mic" {
        let _ = dictation::FIXTURE.set(audio::load_wav(std::path::Path::new(source))?);
    }
    let started = std::time::Instant::now();
    if let Err(error) = dictation::start(start) {
        println!("{}", serde_json::json!({"startError": error}));
        return Ok(());
    }
    std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
    let outcome = dictation::stop(stop)?;
    let mut reply = serde_json::to_value(&outcome).unwrap();
    reply["wall_ms"] = serde_json::json!(started.elapsed().as_millis());
    println!("{reply}");
    Ok(())
}

#[cfg(feature = "e2e")]
fn list_models() -> Result<(), String> {
    let mut models = models::fetch()?;
    models.sort_by(models::compare(settings::Sort::Price));
    for model in models {
        println!(
            "{}",
            serde_json::json!({
                "id": model.id,
                "transcription": model.transcription,
                "pricing": model.pricing,
                "zdr": model.zdr,
                "streaming": model.streaming(),
                "priceText": model.price_text(),
                "priceShort": model.price_short(),
                "menuTitle": model.menu_title(),
            })
        );
    }
    Ok(())
}

#[cfg(feature = "e2e")]
fn test_capture() -> Result<(), String> {
    if permissions::microphone() != permissions::Microphone::Granted
        || !permissions::system_audio_granted()
    {
        return Err("Capture check needs existing microphone and system audio grants.".into());
    }
    let reference = system_audio::SystemAudioSource::start();
    let recorder = audio::Recorder::start()?;
    let chunk = reference
        .chunks
        .recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "No system audio packets arrived within five seconds.")?
        .map_err(|e| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_secs(1));
    let mic = recorder.finish();
    if mic.is_empty() || chunk.samples.is_empty() {
        return Err("Capture returned no audio samples.".into());
    }
    println!(
        "{}",
        serde_json::json!({"microphone_samples": mic.len(), "system_audio_samples": chunk.samples.len(), "sample_rate": audio::SAMPLE_RATE})
    );
    Ok(())
}

#[cfg(feature = "e2e")]
fn transcribe_file(path: &str, model: &str) -> Result<(), String> {
    let samples = audio::load_wav(std::path::Path::new(path))?;
    let key = key::require()?;
    let started = std::time::Instant::now();
    let result = transcribe::transcribe(&samples, model, &key);
    eprintln!("transcribe_ms={}", started.elapsed().as_millis());
    println!("{}", result?);
    Ok(())
}

#[cfg(feature = "e2e")]
fn stream_file(path: &str, model: &str) -> Result<(), String> {
    let samples = audio::load_wav(std::path::Path::new(path))?;
    let key = key::require()?;
    let started = std::time::Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    let session =
        transcribe_stream::Session::start(model.to_string(), key, rx, move |kind, text| {
            println!("{} ms {kind}: {text}", started.elapsed().as_millis());
        })?;
    let mut sent_all = true;
    for (index, chunk) in samples.chunks(1600).enumerate() {
        let due = started + std::time::Duration::from_millis(index as u64 * 100);
        std::thread::sleep(due.saturating_duration_since(std::time::Instant::now()));
        if tx.send(chunk.to_vec()).is_err() {
            sent_all = false;
            break;
        }
    }
    let end = started
        + std::time::Duration::from_secs_f64(samples.len() as f64 / audio::SAMPLE_RATE as f64);
    if sent_all {
        std::thread::sleep(end.saturating_duration_since(std::time::Instant::now()));
    }
    drop(tx);
    let result = session.finish();
    eprintln!("stream_ms={}", started.elapsed().as_millis());
    println!("final: {}", result?);
    Ok(())
}

#[cfg(feature = "e2e")]
fn clean_text(text: &str, model: &str) -> Result<(), String> {
    let key = key::require()?;
    let started = std::time::Instant::now();
    let result = cleanup::cleanup(text, model, None, &key, |_| {});
    eprintln!("cleanup_ms={}", started.elapsed().as_millis());
    let result = result?;
    eprintln!("first_cleanup_text_ms={:?}", result.first_text_ms);
    println!("{}", result.text);
    Ok(())
}

#[cfg(feature = "e2e")]
fn check_key_leaks(paths: &[&str]) -> Result<(), String> {
    fn scan(path: &std::path::Path, prefix: &[u8]) -> Result<bool, String> {
        if path.is_dir() {
            for entry in std::fs::read_dir(path).map_err(|_| "Could not read a scan directory.")? {
                if scan(
                    &entry.map_err(|_| "Could not read a scan entry.")?.path(),
                    prefix,
                )? {
                    return Ok(true);
                }
            }
            Ok(false)
        } else {
            let bytes = std::fs::read(path).map_err(|_| "Could not read a scan file.")?;
            Ok(bytes.windows(prefix.len()).any(|window| window == prefix))
        }
    }
    let key = key::require()?;
    let prefix = &key.as_bytes()[..key.len().min(12)];
    for path in paths {
        if scan(std::path::Path::new(path), prefix)? {
            println!("key prefix match found: true");
            return Err("A key prefix was found in the scanned paths.".into());
        }
    }
    println!("key prefix match found: false");
    Ok(())
}
