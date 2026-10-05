use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::null_mut;

use crate::settings::Shortcut;

pub const COMMAND: u32 = 0x100;
pub const SHIFT: u32 = 0x200;
pub const OPTION: u32 = 0x800;
pub const CONTROL: u32 = 0x1000;

const KEYBOARD_CLASS: u32 = u32::from_be_bytes(*b"keyb");
const HOT_KEY_PRESSED: u32 = 5;
const SIGNATURE: u32 = u32::from_be_bytes(*b"Yapr");
const HOT_KEY_EXISTS: i32 = -9878;

#[repr(C)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

type Handler = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: Handler,
        count: usize,
        types: *const EventTypeSpec,
        user_data: *mut c_void,
        out: *mut *mut c_void,
    ) -> i32;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: *mut c_void,
        options: u32,
        out: *mut *mut c_void,
    ) -> i32;
    fn UnregisterEventHotKey(hot_key: *mut c_void) -> i32;
}

thread_local! {
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
    static REGISTERED: Cell<*mut c_void> = const { Cell::new(null_mut()) };
}

extern "C" fn on_hot_key(_call: *mut c_void, _event: *mut c_void, _data: *mut c_void) -> i32 {
    crate::dictation::toggle();
    0
}

pub fn register(shortcut: &Shortcut) -> Result<(), String> {
    unregister();
    if !INSTALLED.get() {
        let spec = EventTypeSpec {
            event_class: KEYBOARD_CLASS,
            event_kind: HOT_KEY_PRESSED,
        };
        let status = unsafe {
            InstallEventHandler(
                GetApplicationEventTarget(),
                on_hot_key,
                1,
                &spec,
                null_mut(),
                null_mut(),
            )
        };
        if status != 0 {
            return Err(format!("Could not set up the shortcut (error {status})."));
        }
        INSTALLED.set(true);
    }
    let mut hot_key = null_mut();
    let status = unsafe {
        RegisterEventHotKey(
            shortcut.key_code,
            shortcut.modifiers,
            EventHotKeyId {
                signature: SIGNATURE,
                id: 1,
            },
            GetApplicationEventTarget(),
            0,
            &mut hot_key,
        )
    };
    match status {
        0 => {
            REGISTERED.set(hot_key);
            Ok(())
        }
        HOT_KEY_EXISTS => Err(format!(
            "{} is taken by another app. Choose Change Shortcut in the Yapr menu.",
            shortcut.label
        )),
        status => Err(format!(
            "Could not use {} as the shortcut (error {status}).",
            shortcut.label
        )),
    }
}

pub fn unregister() {
    let hot_key = REGISTERED.replace(null_mut());
    if !hot_key.is_null() {
        unsafe { UnregisterEventHotKey(hot_key) };
    }
}

pub fn label(key_code: u16, characters: &str, modifiers: u32) -> String {
    let mut label = String::new();
    for (flag, symbol) in [(CONTROL, "⌃"), (OPTION, "⌥"), (SHIFT, "⇧"), (COMMAND, "⌘")] {
        if modifiers & flag != 0 {
            label.push_str(symbol);
        }
    }
    let key = match key_code {
        49 => "Space".to_string(),
        36 => "Return".into(),
        48 => "Tab".into(),
        51 => "Delete".into(),
        53 => "Esc".into(),
        123 => "←".into(),
        124 => "→".into(),
        125 => "↓".into(),
        126 => "↑".into(),
        code => match function_key(code) {
            Some(number) => format!("F{number}"),
            None => characters.to_uppercase(),
        },
    };
    label.push_str(&key);
    label
}

pub fn function_key(key_code: u16) -> Option<u8> {
    const CODES: [u16; 12] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111];
    CODES
        .iter()
        .position(|&code| code == key_code)
        .map(|index| index as u8 + 1)
}
