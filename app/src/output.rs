use std::ffi::c_void;
use std::ptr::null_mut;

use dispatch2::DispatchQueue;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

use crate::permissions;
use crate::settings::Output;

const KEY_V: u16 = 9;
const FLAG_COMMAND: u64 = 0x0010_0000;
const SESSION_EVENT_TAP: u32 = 1;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateKeyboardEvent(source: *mut c_void, key: u16, down: bool) -> *mut c_void;
    fn CGEventSetFlags(event: *mut c_void, flags: u64);
    fn CGEventPost(tap: u32, event: *mut c_void);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *mut c_void);
}

#[derive(Debug, PartialEq, Eq)]
pub enum Delivered {
    Pasted,
    Copied,
    CopiedWithoutAccessibility,
}

pub fn deliver(text: &str, output: Output) -> Delivered {
    copy(text);
    match output {
        Output::Copy => Delivered::Copied,
        Output::Paste if !permissions::accessibility_granted() => {
            Delivered::CopiedWithoutAccessibility
        }
        Output::Paste => {
            press_paste();
            Delivered::Pasted
        }
    }
}

fn copy(text: &str) {
    let text = text.to_string();
    DispatchQueue::main().exec_sync(move || {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        pasteboard.setString_forType(&NSString::from_str(&text), unsafe {
            NSPasteboardTypeString
        });
    });
}

fn press_paste() {
    for down in [true, false] {
        unsafe {
            let event = CGEventCreateKeyboardEvent(null_mut(), KEY_V, down);
            if event.is_null() {
                return;
            }
            CGEventSetFlags(event, FLAG_COMMAND);
            CGEventPost(SESSION_EVENT_TAP, event);
            CFRelease(event);
        }
    }
}
