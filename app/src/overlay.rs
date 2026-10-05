//! The floating bar. It never takes focus, so typing and the final paste stay with the front app.
//!
//! All AppKit work happens on the main thread; `show` and `level` may be called from any thread.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use dispatch2::{DispatchQueue, DispatchTime};
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSFont, NSFontWeightMedium, NSLineBreakMode, NSPanel, NSScreen,
    NSStatusWindowLevel, NSTextField, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

const WIDTH: f64 = 640.0;
const HEIGHT: f64 = 46.0;
const BOTTOM_MARGIN: f64 = 96.0;
const DOT: f64 = 10.0;

#[derive(Clone, Debug)]
pub enum State {
    /// Recording; the text is the live preview so far.
    Listening(String),
    Transcribing,
    /// Cleanup is running; the text is what the cleanup model has written so far.
    Cleaning(String),
    Done(String),
    Error(String),
}

struct Bar {
    panel: Retained<NSPanel>,
    dot: Retained<NSView>,
    label: Retained<NSTextField>,
}

thread_local! {
    static BAR: RefCell<Option<Bar>> = const { RefCell::new(None) };
}

/// Bumped on every state change, so a pending fade-out never hides a newer state.
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn show(state: State) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    on_main(move |mtm| {
        with_bar(mtm, |bar| bar.apply(&state, mtm));
        let linger = match state {
            State::Done(_) => Some(Duration::from_millis(1500)),
            State::Error(_) => Some(Duration::from_millis(3000)),
            _ => None,
        };
        if let Some(linger) = linger {
            fade_out_after(linger, generation);
        }
    });
}

/// Shows the microphone level (0.0 to 1.0) on the dot while listening.
pub fn level(peak: f32) {
    on_main(move |mtm| {
        with_bar(mtm, |bar| {
            bar.dot
                .setAlphaValue(0.35 + 0.65 * (peak * 4.0).min(1.0) as f64);
        });
    });
}

pub fn on_main(work: impl FnOnce(MainThreadMarker) + Send + 'static) {
    match MainThreadMarker::new() {
        Some(mtm) => work(mtm),
        // SAFETY: the main queue runs its work on the main thread.
        None => DispatchQueue::main()
            .exec_async(move || work(unsafe { MainThreadMarker::new_unchecked() })),
    }
}

fn with_bar(mtm: MainThreadMarker, work: impl FnOnce(&Bar)) {
    BAR.with(|cell| {
        let mut slot = cell.borrow_mut();
        let bar = slot.get_or_insert_with(|| Bar::new(mtm));
        work(bar);
    });
}

/// Waits, then fades the bar out in a few steps unless another state arrived meanwhile.
fn fade_out_after(delay: Duration, generation: u64) {
    const STEPS: u32 = 8;
    for step in 1..=STEPS {
        let when = delay + Duration::from_millis(25 * step as u64);
        let Ok(time) = DispatchTime::try_from(when) else {
            return;
        };
        let _ = DispatchQueue::main().after(time, move || {
            if GENERATION.load(Ordering::SeqCst) != generation {
                return;
            }
            // SAFETY: the main queue runs its work on the main thread.
            let mtm = unsafe { MainThreadMarker::new_unchecked() };
            with_bar(mtm, |bar| {
                if step == STEPS {
                    bar.panel.orderOut(None);
                    bar.panel.setAlphaValue(1.0);
                } else {
                    bar.panel.setAlphaValue(1.0 - step as f64 / STEPS as f64);
                }
            });
        });
    }
}

impl Bar {
    fn new(mtm: MainThreadMarker) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT));
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        // SAFETY: the panel is kept alive by `BAR` for the life of the process.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setFloatingPanel(true);
        panel.setBecomesKeyOnlyIfNeeded(true);
        panel.setHidesOnDeactivate(false);
        panel.setLevel(NSStatusWindowLevel);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        panel.setIgnoresMouseEvents(true);
        panel.setOpaque(false);
        panel.setHasShadow(true);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));

        let background = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
        background.setMaterial(NSVisualEffectMaterial::HUDWindow);
        background.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        background.setState(NSVisualEffectState::Active);
        background.setWantsLayer(true);
        if let Some(layer) = background.layer() {
            layer.setCornerRadius(HEIGHT / 2.0);
            layer.setMasksToBounds(true);
        }

        let dot = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(20.0, (HEIGHT - DOT) / 2.0),
                NSSize::new(DOT, DOT),
            ),
        );
        dot.setWantsLayer(true);
        if let Some(layer) = dot.layer() {
            layer.setCornerRadius(DOT / 2.0);
        }

        let label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        label.setFrame(NSRect::new(
            NSPoint::new(42.0, 13.0),
            NSSize::new(WIDTH - 42.0 - 22.0, 20.0),
        ));
        // SAFETY: a static AppKit constant.
        label.setFont(Some(&NSFont::systemFontOfSize_weight(15.0, unsafe {
            NSFontWeightMedium
        })));
        label.setTextColor(Some(&NSColor::whiteColor()));
        // The newest words matter most, so long text loses its start rather than its end.
        label.setLineBreakMode(NSLineBreakMode::ByTruncatingHead);
        label.setMaximumNumberOfLines(1);

        background.addSubview(&dot);
        background.addSubview(&label);
        panel.setContentView(Some(&background));

        Self { panel, dot, label }
    }

    fn apply(&self, state: &State, mtm: MainThreadMarker) {
        let (color, text) = match state {
            State::Listening(text) => ((1.0, 0.27, 0.27), placeholder(text, "Listening…")),
            State::Transcribing => ((0.55, 0.45, 1.0), "Transcribing…"),
            State::Cleaning(text) => ((0.55, 0.45, 1.0), placeholder(text, "Cleaning up…")),
            State::Done(text) => ((0.25, 0.85, 0.45), placeholder(text, "Done")),
            State::Error(text) => ((1.0, 0.65, 0.15), text.as_str()),
        };
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(color.0, color.1, color.2, 1.0);
        if let Some(layer) = self.dot.layer() {
            layer.setBackgroundColor(Some(&color.CGColor()));
        }
        self.dot.setAlphaValue(1.0);
        self.label
            .setStringValue(&NSString::from_str(&single_line(text)));

        self.place(mtm);
        self.panel.setAlphaValue(1.0);
        self.panel.orderFrontRegardless();
    }

    /// Bottom center of the screen that has the focused window.
    fn place(&self, mtm: MainThreadMarker) {
        let Some(screen) = NSScreen::mainScreen(mtm) else {
            return;
        };
        let visible = screen.visibleFrame();
        let origin = NSPoint::new(
            visible.origin.x + (visible.size.width - WIDTH) / 2.0,
            visible.origin.y + BOTTOM_MARGIN,
        );
        self.panel
            .setFrame_display(NSRect::new(origin, NSSize::new(WIDTH, HEIGHT)), true);
    }
}

fn placeholder<'a>(text: &'a str, empty: &'a str) -> &'a str {
    if text.trim().is_empty() { empty } else { text }
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
