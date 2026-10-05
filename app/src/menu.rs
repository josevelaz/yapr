use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ptr::{NonNull, null_mut};
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSApplication,
    NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSEventMask, NSEventModifierFlags,
    NSFont, NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSModalResponse, NSSecureTextField,
    NSStatusBar, NSStatusItem, NSTextField, NSVariableStatusItemLength, NSWindowStyleMask,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use objc2_service_management::{SMAppService, SMAppServiceStatus};

use crate::dictation::{self, Activity};
use crate::hotkey;
use crate::key;
use crate::models::{self, Model};
use crate::overlay::{self, State};
use crate::permissions::{self, Microphone};
use crate::settings::{self, Output, Settings, Shortcut, Sort};

struct StatusBar {
    item: Retained<NSStatusItem>,
    _menu: Retained<NSMenu>,
    _controller: Retained<Controller>,
}

thread_local! {
    static STATUS_BAR: RefCell<Option<StatusBar>> = const { RefCell::new(None) };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YaprMenuController"]
    struct Controller;

    impl Controller {
        #[unsafe(method(toggleDictation:))]
        fn toggle_dictation(&self, _sender: &NSMenuItem) {
            dictation::toggle();
        }

        #[unsafe(method(grantMicrophone:))]
        fn grant_microphone(&self, _sender: &NSMenuItem) {
            in_background(permissions::request_microphone);
        }

        #[unsafe(method(grantSystemAudio:))]
        fn grant_system_audio(&self, _sender: &NSMenuItem) {
            in_background(permissions::request_system_audio);
        }

        #[unsafe(method(grantAccessibility:))]
        fn grant_accessibility(&self, _sender: &NSMenuItem) {
            in_background(permissions::request_accessibility);
        }

        #[unsafe(method(setApiKey:))]
        fn set_api_key(&self, _sender: &NSMenuItem) {
            let Some(text) = ask_text(
                self.mtm(),
                "AI Gateway API Key",
                "Paste your Vercel AI Gateway API key. Yapr checks it with AI Gateway, then keeps it in your Keychain.",
                "",
                true,
            ) else {
                return;
            };
            in_background(move || key::set(&text).map(|()| "API key saved.".to_string()));
        }

        #[unsafe(method(chooseTranscription:))]
        fn choose_transcription(&self, sender: &NSMenuItem) {
            if let Some(id) = represented_id(sender) {
                settings::update(|s| s.transcription_model = id);
            }
        }

        #[unsafe(method(chooseCleanup:))]
        fn choose_cleanup(&self, sender: &NSMenuItem) {
            let id = represented_id(sender);
            settings::update(|s| s.cleanup_model = id);
        }

        #[unsafe(method(sortByPrice:))]
        fn sort_by_price(&self, _sender: &NSMenuItem) {
            settings::update(|s| s.sort = Sort::Price);
        }

        #[unsafe(method(sortByName:))]
        fn sort_by_name(&self, _sender: &NSMenuItem) {
            settings::update(|s| s.sort = Sort::Name);
        }

        #[unsafe(method(refreshModels:))]
        fn refresh_models(&self, _sender: &NSMenuItem) {
            in_background(|| models::refresh().map(|count| format!("Loaded {count} models.")));
        }

        #[unsafe(method(toggleComputerAudio:))]
        fn toggle_computer_audio(&self, _sender: &NSMenuItem) {
            settings::update(|s| s.remove_computer_audio = !s.remove_computer_audio);
        }

        #[unsafe(method(outputPaste:))]
        fn output_paste(&self, _sender: &NSMenuItem) {
            settings::update(|s| s.output = Output::Paste);
        }

        #[unsafe(method(outputCopy:))]
        fn output_copy(&self, _sender: &NSMenuItem) {
            settings::update(|s| s.output = Output::Copy);
        }

        #[unsafe(method(editInstructions:))]
        fn edit_instructions(&self, _sender: &NSMenuItem) {
            if let Some(text) = ask_text(
                self.mtm(),
                "Extra Instructions",
                "Added to the cleanup prompt, for example names or terms you use often.",
                &settings::get().instructions,
                false,
            ) {
                settings::update(|s| s.instructions = text.trim().to_string());
            }
        }

        #[unsafe(method(changeShortcut:))]
        fn change_shortcut(&self, _sender: &NSMenuItem) {
            change_shortcut(self.mtm());
        }

        #[unsafe(method(toggleLaunchAtLogin:))]
        fn toggle_launch_at_login(&self, _sender: &NSMenuItem) {
            if let Err(error) = toggle_launch_at_login() {
                overlay::show(State::Error(error));
            }
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: &NSMenuItem) {
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }
    }

    unsafe impl NSObjectProtocol for Controller {}

    unsafe impl NSMenuDelegate for Controller {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            self.rebuild(menu);
        }
    }
);

pub fn install(mtm: MainThreadMarker) {
    let controller: Retained<Controller> =
        unsafe { msg_send![super(Controller::alloc(mtm).set_ivars(())), init] };
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    let menu = NSMenu::new(mtm);
    menu.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
    item.setMenu(Some(&menu));
    set_icon(&item, false, mtm);
    STATUS_BAR.set(Some(StatusBar {
        item,
        _menu: menu,
        _controller: controller,
    }));
    if let Err(error) = hotkey::register(&settings::get().shortcut) {
        overlay::show(State::Error(error));
    }
}

pub fn show_recording(recording: bool) {
    overlay::on_main(move |mtm| {
        STATUS_BAR.with_borrow(|bar| {
            if let Some(bar) = bar {
                set_icon(&bar.item, recording, mtm);
            }
        });
    });
}

fn set_icon(item: &NSStatusItem, recording: bool, mtm: MainThreadMarker) {
    let symbol = if recording {
        "waveform.circle.fill"
    } else {
        "waveform"
    };
    let Some(button) = item.button(mtm) else {
        return;
    };
    if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(symbol),
        Some(&NSString::from_str("Yapr")),
    ) {
        image.setTemplate(true);
        button.setImage(Some(&image));
    }
}

fn in_background(work: impl FnOnce() -> Result<String, String> + Send + 'static) {
    std::thread::spawn(move || match work() {
        Ok(message) => overlay::show(State::Done(message)),
        Err(error) => overlay::show(State::Error(error)),
    });
}

fn represented_id(sender: &NSMenuItem) -> Option<String> {
    sender
        .representedObject()?
        .downcast::<NSString>()
        .ok()
        .map(|id| id.to_string())
}

impl Controller {
    fn rebuild(&self, menu: &NSMenu) {
        menu.removeAllItems();
        let settings = settings::get();
        let shortcut = &settings.shortcut.label;
        let toggle = match dictation::activity() {
            Activity::Idle => self.item(
                &format!("Start Dictation ({shortcut})"),
                Some(sel!(toggleDictation:)),
            ),
            Activity::Recording => self.item(
                &format!("Stop Dictation ({shortcut})"),
                Some(sel!(toggleDictation:)),
            ),
            Activity::Busy => self.item("Working…", None),
        };
        menu.addItem(&toggle);
        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        self.add_setup(menu);
        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        self.add_models(menu, &settings);
        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        self.add_options(menu, &settings);
        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        menu.addItem(&self.item("Quit Yapr", Some(sel!(quit:))));
    }

    fn item(&self, title: &str, action: Option<Sel>) -> Retained<NSMenuItem> {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm()),
                &NSString::from_str(title),
                action,
                &NSString::new(),
            )
        };
        if action.is_some() {
            let target: &AnyObject = self.as_ref();
            unsafe { item.setTarget(Some(target)) };
        }
        item
    }

    fn checked(&self, title: &str, action: Option<Sel>, on: bool) -> Retained<NSMenuItem> {
        let item = self.item(title, action);
        item.setState(if on {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        item
    }

    fn add_setup(&self, menu: &NSMenu) {
        menu.addItem(&NSMenuItem::sectionHeaderWithTitle(
            &NSString::from_str("Setup"),
            self.mtm(),
        ));
        let microphone = permissions::microphone() == Microphone::Granted;
        let system_audio = permissions::system_audio_granted();
        let accessibility = permissions::accessibility_granted();
        let key = key::known_present();
        let rows = [
            (
                "Microphone",
                "Allow…",
                microphone,
                sel!(grantMicrophone:),
                "Needed to record your voice",
            ),
            (
                "Screen & System Audio Recording",
                "Allow…",
                system_audio,
                sel!(grantSystemAudio:),
                "Needed for Remove Computer Audio",
            ),
            (
                "Accessibility",
                "Allow…",
                accessibility,
                sel!(grantAccessibility:),
                "Needed to paste into the front app",
            ),
            (
                "AI Gateway API Key",
                "Set…",
                key,
                sel!(setApiKey:),
                "Kept in your macOS Keychain",
            ),
        ];
        for (title, missing, ready, action, tooltip) in rows {
            let title = if ready {
                title.to_string()
            } else {
                format!("{title} — {missing}")
            };
            let item = self.checked(&title, Some(action), ready);
            item.setToolTip(Some(&NSString::from_str(tooltip)));
            menu.addItem(&item);
        }
    }

    fn add_models(&self, menu: &NSMenu, settings: &Settings) {
        let mut all = models::all();
        all.sort_by(models::compare(settings.sort));
        let (transcription, language): (Vec<Model>, Vec<Model>) =
            all.into_iter().partition(|m| m.transcription);
        let name_of = |id: &str| models::find(id).map_or_else(|| id.to_string(), |m| m.name);

        let current = &settings.transcription_model;
        let submenu = NSMenu::new(self.mtm());
        if !transcription.iter().any(|m| &m.id == current) {
            submenu.addItem(&self.checked(current, None, true));
        }
        for model in &transcription {
            submenu.addItem(&self.model_item(
                model,
                sel!(chooseTranscription:),
                &model.id == current,
            ));
        }
        self.add_list_controls(&submenu, settings.sort, transcription.is_empty());
        let item = self.item(&format!("Transcription: {}", name_of(current)), None);
        item.setSubmenu(Some(&submenu));
        menu.addItem(&item);

        let current = settings.cleanup_model.as_deref();
        let submenu = NSMenu::new(self.mtm());
        submenu.addItem(&self.checked("No Cleanup", Some(sel!(chooseCleanup:)), current.is_none()));
        submenu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        let mut providers: BTreeMap<String, Vec<&Model>> = BTreeMap::new();
        for model in &language {
            providers
                .entry(model.provider().to_string())
                .or_default()
                .push(model);
        }
        if let Some(current) = current.filter(|id| !language.iter().any(|m| m.id == *id)) {
            submenu.addItem(&self.checked(current, None, true));
        }
        for (provider, list) in &providers {
            let inner = NSMenu::new(self.mtm());
            let mut contains_current = false;
            for model in list {
                let selected = current == Some(model.id.as_str());
                contains_current |= selected;
                inner.addItem(&self.model_item(model, sel!(chooseCleanup:), selected));
            }
            let item = self.checked(provider, None, contains_current);
            item.setSubmenu(Some(&inner));
            submenu.addItem(&item);
        }
        self.add_list_controls(&submenu, settings.sort, language.is_empty());
        let title = current.map_or_else(|| "No Cleanup".to_string(), name_of);
        let item = self.item(&format!("Cleanup: {title}"), None);
        item.setSubmenu(Some(&submenu));
        menu.addItem(&item);
    }

    fn model_item(&self, model: &Model, action: Sel, selected: bool) -> Retained<NSMenuItem> {
        let item = self.checked(&model.menu_title(), Some(action), selected);
        item.setToolTip(Some(&NSString::from_str(&model.tooltip())));
        let id = NSString::from_str(&model.id);
        unsafe { item.setRepresentedObject(Some(id.as_ref())) };
        item
    }

    fn add_list_controls(&self, menu: &NSMenu, sort: Sort, empty: bool) {
        if empty {
            menu.addItem(&self.item("Could not load the model list", None));
        }
        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        menu.addItem(&self.checked(
            "Sort by Price",
            Some(sel!(sortByPrice:)),
            sort == Sort::Price,
        ));
        menu.addItem(&self.checked("Sort by Name", Some(sel!(sortByName:)), sort == Sort::Name));
        menu.addItem(&self.item("Refresh Model List", Some(sel!(refreshModels:))));
    }

    fn add_options(&self, menu: &NSMenu, settings: &Settings) {
        menu.addItem(&self.checked(
            "Remove Computer Audio",
            Some(sel!(toggleComputerAudio:)),
            settings.remove_computer_audio,
        ));
        let output = NSMenu::new(self.mtm());
        output.addItem(&self.checked(
            "Paste into Front App",
            Some(sel!(outputPaste:)),
            settings.output == Output::Paste,
        ));
        output.addItem(&self.checked(
            "Copy Only",
            Some(sel!(outputCopy:)),
            settings.output == Output::Copy,
        ));
        let item = self.item("Output", None);
        item.setSubmenu(Some(&output));
        menu.addItem(&item);
        menu.addItem(&self.item("Extra Instructions…", Some(sel!(editInstructions:))));
        menu.addItem(&self.item(
            &format!("Change Shortcut ({})…", settings.shortcut.label),
            Some(sel!(changeShortcut:)),
        ));
        let login =
            unsafe { SMAppService::mainAppService().status() } == SMAppServiceStatus::Enabled;
        menu.addItem(&self.checked("Launch at Login", Some(sel!(toggleLaunchAtLogin:)), login));
    }
}

fn toggle_launch_at_login() -> Result<(), String> {
    let service = unsafe { SMAppService::mainAppService() };
    let result = if unsafe { service.status() } == SMAppServiceStatus::Enabled {
        unsafe { service.unregisterAndReturnError() }
    } else {
        unsafe { service.registerAndReturnError() }
    };
    if unsafe { service.status() } == SMAppServiceStatus::RequiresApproval {
        unsafe { SMAppService::openSystemSettingsLoginItems() };
        return Err("Allow Yapr in System Settings → Login Items.".into());
    }
    result.map_err(|error| {
        format!(
            "Could not change Launch at Login: {}",
            error.localizedDescription()
        )
    })
}

fn run_modal(alert: &NSAlert, mtm: MainThreadMarker) -> NSModalResponse {
    alert.layout();
    let window = alert.window();
    window.setStyleMask(window.styleMask() | NSWindowStyleMask::NonactivatingPanel);
    window.center();
    window.makeKeyAndOrderFront(None);
    let app = NSApplication::sharedApplication(mtm);
    app.activate();
    let response = alert.runModal();
    app.deactivate();
    response
}

fn ask_text(
    mtm: MainThreadMarker,
    title: &str,
    info: &str,
    initial: &str,
    secure: bool,
) -> Option<String> {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(info));
    alert.addButtonWithTitle(&NSString::from_str("Save"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    let height = if secure { 24.0 } else { 72.0 };
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(340.0, height));
    let field: Retained<NSTextField> = if secure {
        Retained::into_super(NSSecureTextField::initWithFrame(
            NSSecureTextField::alloc(mtm),
            frame,
        ))
    } else {
        let field = NSTextField::initWithFrame(NSTextField::alloc(mtm), frame);
        if let Some(cell) = field.cell() {
            cell.setWraps(true);
            cell.setScrollable(false);
        }
        field
    };
    field.setStringValue(&NSString::from_str(initial));
    alert.setAccessoryView(Some(&field));
    alert.window().setInitialFirstResponder(Some(&field));
    (run_modal(&alert, mtm) == NSAlertFirstButtonReturn).then(|| field.stringValue().to_string())
}

fn change_shortcut(mtm: MainThreadMarker) {
    let current = settings::get().shortcut;
    hotkey::unregister();
    let chosen = record_shortcut(mtm, &current);
    let result = match chosen {
        Some(shortcut) if shortcut != current => match hotkey::register(&shortcut) {
            Ok(()) => {
                settings::update(|s| s.shortcut = shortcut);
                Ok(())
            }
            Err(error) => {
                let _ = hotkey::register(&current);
                Err(error)
            }
        },
        _ => hotkey::register(&current),
    };
    if let Err(error) = result {
        overlay::show(State::Error(error));
    }
}

fn record_shortcut(mtm: MainThreadMarker, current: &Shortcut) -> Option<Shortcut> {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str("Change Shortcut"));
    alert.setInformativeText(&NSString::from_str(
        "Press the keys you want to use. Include ⌘, ⌥ or ⌃, or use a function key.",
    ));
    alert.addButtonWithTitle(&NSString::from_str("Save"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    alert.addButtonWithTitle(&NSString::from_str(&format!(
        "Use {}",
        Shortcut::default().label
    )));
    let label = NSTextField::labelWithString(&NSString::from_str(&current.label), mtm);
    label.setFont(Some(&NSFont::boldSystemFontOfSize(22.0)));
    label.setFrame(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(260.0, 32.0),
    ));
    alert.setAccessoryView(Some(&label));

    let recorded: Rc<RefCell<Option<Shortcut>>> = Rc::new(RefCell::new(None));
    let seen = recorded.clone();
    let shown = label.clone();
    let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        let event_ref = unsafe { event.as_ref() };
        let flags = event_ref.modifierFlags();
        let mut modifiers = 0;
        for (flag, carbon) in [
            (NSEventModifierFlags::Command, hotkey::COMMAND),
            (NSEventModifierFlags::Option, hotkey::OPTION),
            (NSEventModifierFlags::Control, hotkey::CONTROL),
            (NSEventModifierFlags::Shift, hotkey::SHIFT),
        ] {
            if flags.contains(flag) {
                modifiers |= carbon;
            }
        }
        let key_code = event_ref.keyCode();
        let has_trigger = modifiers & (hotkey::COMMAND | hotkey::OPTION | hotkey::CONTROL) != 0;
        if !has_trigger && hotkey::function_key(key_code).is_none() {
            return if matches!(key_code, 36 | 53 | 76) {
                event.as_ptr()
            } else {
                shown.setStringValue(&NSString::from_str("Add ⌘, ⌥ or ⌃"));
                null_mut()
            };
        }
        let characters = event_ref
            .charactersIgnoringModifiers()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let shortcut = Shortcut {
            key_code: key_code.into(),
            modifiers,
            label: hotkey::label(key_code, &characters, modifiers),
        };
        shown.setStringValue(&NSString::from_str(&shortcut.label));
        *seen.borrow_mut() = Some(shortcut);
        null_mut()
    });
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler)
    };
    let response = run_modal(&alert, mtm);
    if let Some(monitor) = monitor {
        unsafe { NSEvent::removeMonitor(&monitor) };
    }
    let recorded = recorded.borrow_mut().take();
    if response == NSAlertFirstButtonReturn {
        recorded
    } else if response == NSAlertSecondButtonReturn {
        None
    } else {
        Some(Shortcut::default())
    }
}
