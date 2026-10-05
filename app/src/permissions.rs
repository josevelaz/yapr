use std::ffi::c_void;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_foundation::{NSDictionary, NSNumber, NSString};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
}

static SYSTEM_AUDIO_REQUESTED: AtomicBool = AtomicBool::new(false);
static ACCESSIBILITY_REQUESTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Microphone {
    Granted,
    Denied,
    NotDetermined,
}

pub fn microphone() -> Microphone {
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(AVMediaTypeAudio.unwrap()) } {
        AVAuthorizationStatus::Authorized => Microphone::Granted,
        AVAuthorizationStatus::NotDetermined => Microphone::NotDetermined,
        _ => Microphone::Denied,
    }
}

pub fn system_audio_granted() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

pub fn accessibility_granted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

fn open_settings(pane: &str) -> Result<(), String> {
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{pane}");
    match std::process::Command::new("/usr/bin/open")
        .arg(url)
        .status()
    {
        Ok(status) if status.success() => Ok(()),
        _ => Err("Could not open System Settings. Open Privacy & Security manually.".into()),
    }
}

pub fn request_microphone() -> Result<String, String> {
    match microphone() {
        Microphone::Granted => Ok("Microphone access is already allowed.".into()),
        Microphone::Denied => {
            open_settings("Privacy_Microphone")?;
            Ok("Allow microphone access for Yapr in System Settings.".into())
        }
        Microphone::NotDetermined => {
            let (tx, rx) = mpsc::channel();
            DispatchQueue::main().exec_async(move || {
                let handler = RcBlock::new(move |_: Bool| {
                    let _ = tx.send(());
                });
                unsafe {
                    AVCaptureDevice::requestAccessForMediaType_completionHandler(
                        AVMediaTypeAudio.unwrap(),
                        &handler,
                    );
                }
            });
            let answered = rx.recv_timeout(Duration::from_secs(60)).is_ok();
            Ok(match microphone() {
                Microphone::Granted => "Microphone access allowed.",
                Microphone::Denied => "Microphone access denied. Allow it in System Settings.",
                Microphone::NotDetermined if !answered => {
                    "Answer the microphone prompt, then try again."
                }
                Microphone::NotDetermined => "Check microphone access again.",
            }
            .into())
        }
    }
}

pub fn request_system_audio() -> Result<String, String> {
    if system_audio_granted() {
        return Ok("Screen & System Audio Recording is already allowed.".into());
    }
    if SYSTEM_AUDIO_REQUESTED.swap(true, Ordering::SeqCst) {
        open_settings("Privacy_ScreenCapture")?;
        return Ok(
            "Allow Screen & System Audio Recording for Yapr, then quit and reopen it.".into(),
        );
    }
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("system-audio-permission".into())
        .spawn(move || {
            let _ = tx.send(unsafe { CGRequestScreenCaptureAccess() });
        })
        .map_err(|_| "Could not request Screen & System Audio Recording access.")?;
    let _ = rx.recv_timeout(Duration::from_secs(2));
    Ok(if system_audio_granted() {
        "Screen & System Audio Recording allowed."
    } else {
        "Allow Screen & System Audio Recording for Yapr, then quit and reopen it."
    }
    .into())
}

pub fn request_accessibility() -> Result<String, String> {
    if accessibility_granted() {
        return Ok("Accessibility is already allowed.".into());
    }
    if ACCESSIBILITY_REQUESTED.swap(true, Ordering::SeqCst) {
        open_settings("Privacy_Accessibility")?;
    } else {
        let options: Retained<NSDictionary<NSString, NSNumber>> = NSDictionary::from_slices(
            &[&*NSString::from_str("AXTrustedCheckOptionPrompt")],
            &[&*NSNumber::new_bool(true)],
        );
        unsafe { AXIsProcessTrustedWithOptions(Retained::as_ptr(&options).cast()) };
    }
    Ok("Allow Accessibility for Yapr in System Settings so it can paste.".into())
}
