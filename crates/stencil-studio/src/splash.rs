//! Startup animation: the hummingbird GIF, decoded off the UI thread and played once,
//! then faded into the app. Clicking or tapping skips it.

use crate::AppWindow;
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode};
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    static SPLASH: Rc<RefCell<Splash>> = Rc::new(RefCell::new(Splash::default()));
}
use std::time::Duration;

static GIF: &[u8] = include_bytes!("../ui/splash.gif");
/// Never keep the user waiting longer than this, even if decoding is slow.
const MAX_WAIT: Duration = Duration::from_millis(3500);

fn decode() -> Vec<(SharedPixelBuffer<Rgba8Pixel>, Duration)> {
    use image::AnimationDecoder;
    let Ok(dec) = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(GIF)) else {
        return Vec::new();
    };
    dec.into_frames()
        .filter_map(Result::ok)
        .map(|f| {
            let (n, d) = f.delay().numer_denom_ms();
            let ms = (n as f32 / d.max(1) as f32).max(20.0);
            let img = f.into_buffer();
            (
                SharedPixelBuffer::clone_from_slice(img.as_raw(), img.width(), img.height()),
                Duration::from_millis(ms as u64),
            )
        })
        .collect()
}

pub fn start(ui: &AppWindow) {
    ui.set_splash_visible(true);
    let state = SPLASH.with(|s| s.clone());

    // safety net: always finish
    let w = ui.as_weak();
    let st = state.clone();
    Timer::single_shot(MAX_WAIT, move || {
        if let Some(ui) = w.upgrade() {
            finish(&ui, &st);
        }
    });
    let w = ui.as_weak();
    let st = state.clone();
    ui.on_splash_skip(move || {
        if let Some(ui) = w.upgrade() {
            finish(&ui, &st);
        }
    });

    let w = ui.as_weak();
    let deliver = move |frames: Vec<(SharedPixelBuffer<Rgba8Pixel>, Duration)>| {
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = w.upgrade() else { return };
            let st = SPLASH.with(|s| s.clone());
            if frames.is_empty() || st.borrow().done {
                finish(&ui, &st);
                return;
            }
            let frames: Vec<(Image, Duration)> = frames.into_iter().map(|(b, d)| (Image::from_rgba8(b), d)).collect();
            play(&ui, st, Rc::new(frames), 0);
        });
    };
    std::thread::spawn(move || deliver(decode()));
}

#[derive(Default)]
struct Splash {
    done: bool,
    timer: Option<Timer>,
}

fn play(ui: &AppWindow, st: Rc<RefCell<Splash>>, frames: Rc<Vec<(Image, Duration)>>, i: usize) {
    if st.borrow().done {
        return;
    }
    let Some((img, delay)) = frames.get(i) else {
        finish(ui, &st);
        return;
    };
    ui.set_splash_frame(img.clone());
    let w = ui.as_weak();
    let st2 = st.clone();
    let t = Timer::default();
    t.start(TimerMode::SingleShot, *delay, move || {
        if let Some(ui) = w.upgrade() {
            play(&ui, st2.clone(), frames.clone(), i + 1);
        }
    });
    st.borrow_mut().timer = Some(t);
}

fn finish(ui: &AppWindow, st: &Rc<RefCell<Splash>>) {
    {
        let mut s = st.borrow_mut();
        if s.done {
            return;
        }
        s.done = true;
        s.timer = None;
    }
    ui.set_splash_fading(true);
    let w = ui.as_weak();
    Timer::single_shot(Duration::from_millis(400), move || {
        if let Some(ui) = w.upgrade() {
            ui.set_splash_visible(false);
            ui.set_splash_frame(Image::default()); // free the frames
        }
    });
}
