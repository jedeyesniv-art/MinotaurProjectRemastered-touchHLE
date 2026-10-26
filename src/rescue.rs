/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Private fork: `--rescue-pad`, controller support for Minotaur Rescue
//! (iOS) that follows what's on screen. Buttons tap the menus' buttons; in
//! play the left stick drives a virtual finger that strokes the way the
//! stick points, like the PC beta's TOUCHY `touch_stick` (ported from
//! `tools\d3d9-limiter\d3d9_limiter.c`, with the user's settled tuning).
//!
//! Screens are told apart by colour, measured on the user's screenshots
//! (landscape 1024x768 points as the user sees them):
//! - pause: the purple UNPAUSE and dark red QUIT GAME buttons
//! - level select: the SURVIVAL button (dark blue, or red when chosen) and
//!   the row of level boxes
//! - info: the yellow ABOUT SOLAR MINOTAUR RESCUE FRENZY in the middle
//! - main menu: the yellow game name at the top and cyan PLAY
//! - title: the red top band of the ATAURUS rainbow
//! - the info screen, main menu and level select also need the purple band
//!   under their headers (0.32-0.57; at most 0.08 in play): a yellow
//!   explosion in play once passed for the info screen
//! - text pages (from info): the green box with a teal deer, their BACK
//!
//! Play looks like the attract demo, so it's known by state instead: from
//! leaving the level select (by pad or mouse) until a menu or the title
//! comes back, seen in two checks running (game over is a tap in the
//! middle, so A gives one). Other screens count
//! as the attract demo.

use crate::frameworks::opengles::eagl::read_screen_pixels;
use crate::frameworks::uikit::{ui_application, ui_touch};
use crate::gridrunner::{fraction, from_view_point, from_view_rect};
use crate::mem::Ptr;
use crate::window::{Event, FingerId, PadInput};
use crate::Environment;
use sdl2::controller::Button;
use std::cell::RefCell;
use std::collections::HashMap;

/// The ship's turn (armv7 1.0, ARM code): each frame its angle eases toward
/// the heading the stroke sets, `atan2(-dy, dx) - pi/2`, by a fraction of
/// the difference, wrapped to take the short way round. A difference under
/// -pi gets 2pi added, but one over pi becomes `2pi - diff` instead of
/// `diff - 2pi` (`vsub.f64 d7, d6, d7` here), turning the wrong way and
/// on round the long way. Strokes to the left sit on atan2's seam at +-pi,
/// so the slightest vertical wobble set it off.
const TURN_WRAP: u32 = 0x3f228;
const TURN_WRAP_GAME: u32 = 0xee367b47; // vsub.f64 d7, d6, d7
const TURN_WRAP_FIXED: u32 = 0xee377b46; // vsub.f64 d7, d7, d6

/// `--rescue-turn-fix`: make the ship turn the short way round.
pub fn apply_patch(env: &mut Environment) {
    if !env.options.rescue_turn_fix || env.bundle.bundle_identifier() != "uk.co.llamasoft.minorescue" {
        return;
    }
    let at = Ptr::<u32, true>::from_bits(TURN_WRAP);
    if env.mem.read(at) != TURN_WRAP_GAME {
        log!("--rescue-turn-fix: unexpected code, is this Minotaur Rescue 1.0? Not patching.");
        return;
    }
    env.mem.write(at, TURN_WRAP_FIXED);
    env.cpu.invalidate_cache_range(TURN_WRAP, 4);
    log!("--rescue-turn-fix: the ship turns the short way round.");
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Screen {
    Title,
    MainMenu,
    Info,
    TextPage,
    LevelSelect,
    Play,
    Pause,
    Attract,
}

type Rect = (u32, u32, u32, u32);
const UNPAUSE_BOX: Rect = (330, 165, 350, 85);
const QUIT_BOX: Rect = (330, 420, 350, 85);
const SURVIVAL_BOX: Rect = (330, 420, 350, 85);
const LEVEL_ROW: Rect = (100, 200, 780, 80);
const INFO_HEADING: Rect = (300, 250, 420, 45);
const MENU_NAME: Rect = (300, 40, 420, 45);
const MENU_PLAY: Rect = (690, 660, 110, 45);
const RAINBOW_RED: Rect = (0, 32, 1024, 25);
const MENU_BAND: Rect = (0, 130, 1024, 25);
const TEXT_DEER: Rect = (890, 590, 80, 80);

// Where to tap (landscape points).
const MIDDLE: (f32, f32) = (512.0, 384.0);
const SUN: (f32, f32) = (512.0, 384.0);
const MENU_INFO: (f32, f32) = (259.0, 686.0);
const MENU_PLAY_TAP: (f32, f32) = (737.0, 686.0);
const MENU_SELECT_GAME: (f32, f32) = (512.0, 60.0);
const INFO_ABOUT_GAME: (f32, f32) = (512.0, 273.0);
const INFO_ABOUT_LLAMASOFT: (f32, f32) = (242.0, 82.0);
const INFO_MINOTAUR_PROJECT: (f32, f32) = (761.0, 82.0);
const BACK: (f32, f32) = (256.0, 686.0);
const TEXT_BACK: (f32, f32) = (930.0, 635.0);
const LEVEL_START: (f32, f32) = (748.0, 686.0);
const LEVEL_SURVIVAL: (f32, f32) = (509.0, 463.0);
const UNPAUSE: (f32, f32) = (509.0, 208.0);
const QUIT_GAME: (f32, f32) = (509.0, 464.0);
/// The centres of the eight boxes in the level select's row (y 240) and the
/// pause screen's audio level row (y 592).
const BOX_X: [f32; 8] = [137.0, 240.0, 343.0, 446.0, 548.0, 651.0, 754.0, 857.0];
const LEVEL_ROW_Y: f32 = 240.0;
const AUDIO_ROW_Y: f32 = 592.0;

/// The virtual finger's tuning: the PC beta's ini as the user settled it.
const TOUCH_AREA: (f32, f32, f32, f32) = (0.55, 0.55, 0.97, 0.95);
const TOUCH_DEAD: f32 = 0.2;
const TOUCH_STEP: f32 = 3.0; // the beta's 8 pixels of 2160, in points
const TOUCH_CURVE: f32 = 2.0;
const TOUCH_START: f32 = 0.25;
const TOUCH_RAMP: f32 = 0.25;
const TOUCH_TURN: f32 = 35.0;
const TOUCH_SMOOTH: f32 = 0.06;
const TOUCH_RELEASE: f32 = 0.0;
const TOUCH_FLIP: f32 = 170.0;
const VIEW: (f32, f32) = (1024.0, 768.0);
const DT: f32 = 1.0 / 60.0;
/// Text pages: the stick drags the text up to this fast (points a second),
/// with a finger between these heights.
const SCROLL_SPEED: f32 = 700.0;
const SCROLL_TOP: f32 = 150.0;
const SCROLL_BOTTOM: f32 = 620.0;

fn score(env: &mut Environment, rect: Rect, test: impl Fn(u8, u8, u8) -> bool) -> f32 {
    let rect = from_view_rect(env, rect);
    read_screen_pixels(env, rect).map_or(0.0, |p| fraction(&p, test))
}

fn red(r: u8, g: u8, b: u8) -> bool {
    r > 180 && g < 70 && b < 70
}

/// The screen judged by its colours alone (`None`: play or the attract
/// demo).
fn recognise_colours(env: &mut Environment) -> Option<Screen> {
    let purple = |r: u8, g: u8, b: u8| (15..130).contains(&r) && g < 40 && (60..180).contains(&b);
    let dark_red = |r: u8, g: u8, b: u8| (45..170).contains(&r) && g < 35 && b < 35;
    let dark_blue = |r: u8, g: u8, b: u8| r < 35 && g < 35 && (40..130).contains(&b);
    let yellow = |r: u8, g: u8, b: u8| r > 140 && g > 170 && b < 90;
    let cyan = |r: u8, g: u8, b: u8| r < 120 && g > 150 && b > 150;
    let dark_green = |r: u8, g: u8, b: u8| r < 40 && (20..80).contains(&g) && b < 40;
    let teal = |r: u8, g: u8, b: u8| r < 90 && g > 130 && (90..220).contains(&b);
    if score(env, TEXT_DEER, dark_green) > 0.3 && score(env, TEXT_DEER, teal) > 0.08 {
        return Some(Screen::TextPage);
    }
    let scores = [
        score(env, UNPAUSE_BOX, purple),
        score(env, QUIT_BOX, dark_red),
        score(env, SURVIVAL_BOX, dark_blue),
        score(env, LEVEL_ROW, dark_blue),
        score(env, INFO_HEADING, yellow),
        score(env, MENU_NAME, yellow),
        score(env, MENU_PLAY, cyan),
        score(env, RAINBOW_RED, red),
        score(env, MENU_BAND, purple),
    ];
    log_dbg!("Rescue pad scores: {:?}", scores);
    let [unpause, quit, survival, row, info, name, play, rainbow, band] = scores;
    let menu = band > 0.15;
    if unpause > 0.25 && quit > 0.25 {
        Some(Screen::Pause)
    } else if menu && (survival > 0.4 || score(env, SURVIVAL_BOX, red) > 0.4) && row > 0.25 {
        Some(Screen::LevelSelect)
    } else if menu && info > 0.04 {
        Some(Screen::Info)
    } else if menu && name > 0.04 && play > 0.02 {
        Some(Screen::MainMenu)
    } else if rainbow > 0.5 {
        Some(Screen::Title)
    } else {
        None
    }
}

#[derive(Default)]
struct State {
    frames: u32,
    /// Set when the level select or pause screen gives way to an
    /// unrecognised screen, cleared by any recognised menu.
    playing: bool,
    /// The last screen recognised by colour.
    last_known: Option<Screen>,
    /// In play, a menu seen once: play ends if the next check sees it too.
    leaving: Option<Screen>,
    /// The screen at the last check (every few frames).
    screen: Option<Screen>,
    /// Touches held by inputs, until they're released.
    held: HashMap<PadInput, (FingerId, (f32, f32))>,
    /// The double tap on the sun that pauses: frames to go, and each step
    /// (true = down).
    pause_taps: Vec<(u32, bool)>,
    /// Frames to keep the finger off (after a pause tap).
    finger_off: u32,
    finger: Finger,
    /// The text page scroll finger's height, while it's down.
    scroll: Option<f32>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn screen_now(env: &mut Environment) -> Screen {
    let colours = recognise_colours(env);
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let leaving = std::mem::take(&mut s.leaving);
        if let Some(screen) = colours {
            if s.playing && screen != Screen::Pause && leaving != Some(screen) {
                // Once could be noise in play.
                s.leaving = Some(screen);
                return Screen::Play;
            }
            if screen != Screen::Pause {
                s.playing = false;
            }
            s.last_known = Some(screen);
            return screen;
        }
        if matches!(s.last_known, Some(Screen::LevelSelect | Screen::Pause)) {
            s.playing = true;
        }
        if s.playing {
            Screen::Play
        } else {
            Screen::Attract
        }
    })
}

enum Action {
    Tap((f32, f32)),
    /// A tap on the box next to the selected one in a row of eight.
    Step { row_y: f32, by: i32 },
    Pause,
    Quit,
    Nothing,
}

fn action(screen: Screen, input: PadInput) -> Action {
    use Button as B;
    use PadInput as P;
    let left = matches!(input, P::Button(B::DPadLeft) | P::StickLeft);
    let right = matches!(input, P::Button(B::DPadRight) | P::StickRight);
    let button = match input {
        P::Button(button) => Some(button),
        _ => None,
    };
    match (screen, button) {
        (Screen::Title | Screen::Attract, Some(B::A | B::Start)) => Action::Tap(MIDDLE),
        (Screen::Title | Screen::Attract | Screen::MainMenu, Some(B::Back)) => Action::Quit,
        (Screen::MainMenu, Some(B::A | B::Start)) => Action::Tap(MENU_PLAY_TAP),
        (Screen::MainMenu, Some(B::X)) => Action::Tap(MENU_INFO),
        (Screen::MainMenu, Some(B::Y)) => Action::Tap(MENU_SELECT_GAME),
        (Screen::Info, Some(B::A)) => Action::Tap(INFO_ABOUT_GAME),
        (Screen::Info, Some(B::X)) => Action::Tap(INFO_ABOUT_LLAMASOFT),
        (Screen::Info, Some(B::Y)) => Action::Tap(INFO_MINOTAUR_PROJECT),
        (Screen::Info, Some(B::B | B::Back)) => Action::Tap(BACK),
        (Screen::TextPage, Some(B::A | B::B | B::Back)) => Action::Tap(TEXT_BACK),
        (Screen::LevelSelect, Some(B::A | B::Start)) => Action::Tap(LEVEL_START),
        (Screen::LevelSelect, Some(B::B | B::Back)) => Action::Tap(BACK),
        (Screen::LevelSelect, Some(B::X)) => Action::Tap(LEVEL_SURVIVAL),
        (Screen::LevelSelect, _) if left => Action::Step { row_y: LEVEL_ROW_Y, by: -1 },
        (Screen::LevelSelect, _) if right => Action::Step { row_y: LEVEL_ROW_Y, by: 1 },
        (Screen::Play, Some(B::Start)) => Action::Pause,
        (Screen::Play, Some(B::A)) => Action::Tap(MIDDLE),
        (Screen::Pause, Some(B::Start)) => Action::Tap(UNPAUSE),
        (Screen::Pause, Some(B::Back)) => Action::Tap(QUIT_GAME),
        (Screen::Pause, _) if left => Action::Step { row_y: AUDIO_ROW_Y, by: -1 },
        (Screen::Pause, _) if right => Action::Step { row_y: AUDIO_ROW_Y, by: 1 },
        _ => Action::Nothing,
    }
}

/// Which of a row's eight boxes is selected (red), if any.
fn selected_box(env: &mut Environment, row_y: f32) -> Option<usize> {
    let scores: Vec<f32> = BOX_X
        .iter()
        .map(|&x| score(env, (x as u32 - 30, row_y as u32 - 40, 60, 80), red))
        .collect();
    let (best, &most) = scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    (most > 0.3).then_some(best)
}

fn finger_for(input: PadInput) -> FingerId {
    let index = match input {
        PadInput::Button(button) => button as u8,
        PadInput::LeftTrigger => 100,
        PadInput::RightTrigger => 101,
        PadInput::StickLeft => 102,
        PadInput::StickRight => 103,
    };
    FingerId::Pad(index)
}

const STROKE_FINGER: FingerId = FingerId::Pad(200);
const PAUSE_FINGER: FingerId = FingerId::Pad(201);
const SCROLL_FINGER: FingerId = FingerId::Pad(202);

fn touch(env: &mut Environment, finger: FingerId, point: (f32, f32), phase: u8) {
    let at = from_view_point(env, point);
    let touches = HashMap::from([(finger, at)]);
    ui_touch::handle_event(
        env,
        match phase {
            0 => Event::TouchesDown(touches),
            1 => Event::TouchesMove(touches),
            _ => Event::TouchesUp(touches),
        },
    );
}

pub fn handle_input(env: &mut Environment, input: PadInput, down: bool) {
    if !down {
        if let Some((finger, at)) = STATE.with(|s| s.borrow_mut().held.remove(&input)) {
            ui_touch::handle_event(env, Event::TouchesUp(HashMap::from([(finger, at)])));
        }
        return;
    }
    let screen = screen_now(env);
    let action = action(screen, input);
    if !matches!((input, &action), (PadInput::StickLeft | PadInput::StickRight, Action::Nothing)) {
        log!("Rescue pad: {:?} on {:?}", input, screen);
    }
    let point = match action {
        Action::Tap(point) => point,
        Action::Step { row_y, by } => {
            // None selected (SURVIVAL chosen instead): start at level 1.
            let next = match selected_box(env, row_y) {
                Some(current) => (current as i32 + by).clamp(0, 7) as usize,
                None => 0,
            };
            (BOX_X[next], row_y)
        }
        Action::Pause => {
            lift_finger(env, "pause");
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.pause_taps = vec![(0, true), (3, false), (8, true), (11, false)];
                s.finger_off = 60;
            });
            return;
        }
        Action::Quit => {
            echo!("B pressed on Minotaur Rescue's title or main menu, exiting.");
            ui_application::exit(env);
            return;
        }
        Action::Nothing => return,
    };
    let at = from_view_point(env, point);
    let finger = finger_for(input);
    STATE.with(|s| s.borrow_mut().held.insert(input, (finger, at)));
    ui_touch::handle_event(env, Event::TouchesDown(HashMap::from([(finger, at)])));
}

/// Called each frame: the pause taps, a screen check every few frames, and
/// the virtual finger.
pub fn frame(env: &mut Environment) {
    let (n, taps) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.frames += 1;
        s.finger_off = s.finger_off.saturating_sub(1);
        let mut due = Vec::new();
        s.pause_taps.retain_mut(|(wait, down)| {
            if *wait == 0 {
                due.push(*down);
                false
            } else {
                *wait -= 1;
                true
            }
        });
        (s.frames, due)
    });
    for down in taps {
        touch(env, PAUSE_FINGER, SUN, if down { 0 } else { 2 });
    }
    if n % 10 == 0 {
        let screen = screen_now(env);
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            if s.screen != Some(screen) {
                log!("Rescue pad: screen {:?}", screen);
            }
            s.screen = Some(screen);
        });
        // Play's black sky and colourful noise make a messy glow: black bars
        // there (and the attract demo, which is play), the glow on the title
        // and menus.
        let black = matches!(screen, Screen::Play | Screen::Pause | Screen::Attract);
        env.window_mut().set_ambient_bars_off(black);
    }
    finger_frame(env);
    scroll_frame(env);
}

/// Text pages: the stick's up and down drag the text, pushing down reading
/// on (the finger drags up). At the edge of its room the finger lifts and
/// goes down again at the other end.
fn scroll_frame(env: &mut Environment) {
    let on_page = STATE.with(|s| s.borrow().screen == Some(Screen::TextPage));
    let sy = if on_page { env.window().left_stick_raw().1 } else { 0.0 };
    let was = STATE.with(|s| s.borrow().scroll);
    if sy.abs() < TOUCH_DEAD {
        if let Some(y) = was {
            touch(env, SCROLL_FINGER, (512.0, y), 2);
            STATE.with(|s| s.borrow_mut().scroll = None);
        }
        return;
    }
    let strength = ((sy.abs() - TOUCH_DEAD) / (1.0 - TOUCH_DEAD)).min(1.0).powf(1.5);
    match was {
        None => {
            let y = if sy > 0.0 { SCROLL_BOTTOM } else { SCROLL_TOP };
            touch(env, SCROLL_FINGER, (512.0, y), 0);
            STATE.with(|s| s.borrow_mut().scroll = Some(y));
        }
        Some(y) => {
            let y = y - sy.signum() * strength * SCROLL_SPEED * DT;
            if !(SCROLL_TOP..=SCROLL_BOTTOM).contains(&y) {
                touch(env, SCROLL_FINGER, (512.0, y.clamp(SCROLL_TOP, SCROLL_BOTTOM)), 2);
                STATE.with(|s| s.borrow_mut().scroll = None);
            } else {
                touch(env, SCROLL_FINGER, (512.0, y), 1);
                STATE.with(|s| s.borrow_mut().scroll = Some(y));
            }
        }
    }
}

/// The virtual finger (in landscape points) and the stick between frames:
/// the smoothed stick, the direction the current ramp is measured against,
/// how long the ramp has run, and the let-go hold.
#[derive(Default)]
struct Finger {
    down: bool,
    placed: bool,
    x: f32,
    y: f32,
    sent: (f32, f32),
    pushing: bool,
    smooth: (f32, f32),
    reference: (f32, f32),
    ramp_time: f32,
    /// Seconds the stick has been centred mid-push (None = not).
    released_for: Option<f32>,
    /// The stroke's direction when the stick was let go.
    last: (f32, f32),
}

fn finger_active() -> bool {
    STATE.with(|s| {
        let s = s.borrow();
        s.screen == Some(Screen::Play) && s.finger_off == 0 && s.pause_taps.is_empty()
    })
}

fn finger_send(env: &mut Environment, phase: u8) {
    let (x, y) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let f = &mut s.finger;
        f.sent = (f.x.round(), f.y.round());
        f.sent
    });
    touch(env, STROKE_FINGER, (x, y), phase);
}

fn lift_finger(env: &mut Environment, why: &str) {
    let was_down = STATE.with(|s| std::mem::take(&mut s.borrow_mut().finger.down));
    if was_down {
        finger_send(env, 2);
        log_dbg!("Rescue pad finger: lift ({})", why);
    }
}

fn finger_frame(env: &mut Environment) {
    let active = finger_active();
    let (mut sx, mut sy) = if active { env.window().left_stick_raw() } else { (0.0, 0.0) };
    let mut mag = (sx * sx + sy * sy).sqrt();
    let (down, pushing, released_for) = STATE.with(|s| {
        let f = &s.borrow().finger;
        (f.down, f.pushing, f.released_for)
    });
    if mag < TOUCH_DEAD {
        if pushing && active && down {
            // Through the centre: hold the finger a moment, to see whether
            // this is a flip, a turn or a let-go.
            let held = STATE.with(|s| {
                let f = &mut s.borrow_mut().finger;
                if f.released_for.is_none() {
                    let len = (f.smooth.0 * f.smooth.0 + f.smooth.1 * f.smooth.1).sqrt();
                    f.last = if len > 0.0 { (f.smooth.0 / len, f.smooth.1 / len) } else { (0.0, 0.0) };
                }
                let t = f.released_for.unwrap_or(0.0) + DT;
                f.released_for = Some(t);
                t
            });
            if held < TOUCH_RELEASE {
                return;
            }
            lift_finger(env, "stick let go");
        } else {
            lift_finger(env, if active { "stick let go" } else { "menu or pause" });
        }
        STATE.with(|s| {
            let f = &mut s.borrow_mut().finger;
            f.pushing = false;
            f.released_for = None;
        });
        return;
    }
    if mag > 1.0 {
        sx /= mag;
        sy /= mag;
        mag = 1.0;
    }
    let raw = (sx / mag, sy / mag);
    if released_for.is_some() {
        // Pushed again just after passing through the centre.
        let last = STATE.with(|s| {
            let f = &mut s.borrow_mut().finger;
            f.released_for = None;
            f.last
        });
        let angle = (raw.0 * last.0 + raw.1 * last.1).clamp(-1.0, 1.0).acos().to_degrees();
        if angle >= TOUCH_FLIP {
            lift_finger(env, "flip");
            STATE.with(|s| s.borrow_mut().finger.pushing = false);
        }
    }

    let (dx, dy, factor) = STATE.with(|s| {
        let f = &mut s.borrow_mut().finger;
        if !f.pushing {
            // A new push: start smoothing and the ramp from here.
            f.pushing = true;
            f.smooth = (sx, sy);
            f.reference = raw;
            f.ramp_time = 0.0;
        } else {
            // Smooth the stick, so small wobbles don't jerk the stroke.
            let a = 1.0 - (-DT / TOUCH_SMOOTH).exp();
            f.smooth.0 += (sx - f.smooth.0) * a;
            f.smooth.1 += (sy - f.smooth.1) * a;
            // A sharp change of direction restarts the ramp. The reference
            // follows the stick slowly, so sweeping round gently doesn't.
            let cosine = raw.0 * f.reference.0 + raw.1 * f.reference.1;
            if cosine < TOUCH_TURN.to_radians().cos() {
                f.ramp_time = 0.0;
                f.reference = raw;
            } else {
                let b = 1.0 - (-DT / 0.15).exp();
                f.reference.0 += (raw.0 - f.reference.0) * b;
                f.reference.1 += (raw.1 - f.reference.1) * b;
                let len = (f.reference.0 * f.reference.0 + f.reference.1 * f.reference.1).sqrt();
                if len > 0.0 {
                    f.reference = (f.reference.0 / len, f.reference.1 / len);
                }
                f.ramp_time += DT;
            }
        }
        let smag = (f.smooth.0 * f.smooth.0 + f.smooth.1 * f.smooth.1).sqrt();
        if smag < 1e-6 {
            return (0.0, 0.0, None);
        }
        let strength = ((mag - TOUCH_DEAD) / (1.0 - TOUCH_DEAD)).powf(TOUCH_CURVE);
        // Soft friction: slow at first, easing (smoothstep) up to full speed.
        let ramp = (f.ramp_time / TOUCH_RAMP).min(1.0);
        let factor = TOUCH_START + (1.0 - TOUCH_START) * ramp * ramp * (3.0 - 2.0 * ramp);
        (f.smooth.0 / smag, f.smooth.1 / smag, Some(strength * factor))
    });
    let Some(speed) = factor else { return };

    let (ax0, ay0) = (TOUCH_AREA.0 * VIEW.0, TOUCH_AREA.1 * VIEW.1);
    let (ax1, ay1) = (TOUCH_AREA.2 * VIEW.0, TOUCH_AREA.3 * VIEW.1);
    let area_size = (ax1 - ax0).min(ay1 - ay0);
    let inside = |x: f32, y: f32| x >= ax0 && x <= ax1 && y >= ay0 && y <= ay1;
    let touch_speed = env.options.rescue_touch_speed;
    if STATE.with(|s| s.borrow().finger.down) {
        let (x, y, sent) = STATE.with(|s| {
            let f = &mut s.borrow_mut().finger;
            let step = touch_speed * VIEW.1 * speed * DT;
            f.x += dx * step;
            f.y += dy * step;
            (f.x, f.y, f.sent)
        });
        if !inside(x, y) {
            // Out of room: lift, and start a new stroke next frame (the ramp
            // carries on: it's still the same push).
            lift_finger(env, "edge, restarting the stroke");
            return;
        }
        // Report the move once it's big enough to have a clear direction.
        let (mx, my) = (x - sent.0, y - sent.1);
        if mx * mx + my * my >= TOUCH_STEP * TOUCH_STEP {
            finger_send(env, 1);
        }
    } else {
        // A new stroke starts where the last one ended, like a real finger,
        // if there's room ahead; otherwise behind the centre. The game only
        // sees the touch go down there, so the jump isn't a stroke.
        STATE.with(|s| {
            let f = &mut s.borrow_mut().finger;
            let room = 0.4 * area_size;
            let fits = f.placed && inside(f.x, f.y) && inside(f.x + dx * room, f.y + dy * room);
            if !fits {
                f.x = (ax0 + ax1) / 2.0 - dx * area_size * 0.4;
                f.y = (ay0 + ay1) / 2.0 - dy * area_size * 0.4;
            }
            f.placed = true;
            f.down = true;
        });
        finger_send(env, 0);
        // The game zeroes a new touch's drag vector, and until it moves
        // steers the ship by atan2 of that zero: towards the right, so a
        // slow push left swung the ship round for a few frames. Make the
        // first move at once.
        STATE.with(|s| {
            let f = &mut s.borrow_mut().finger;
            f.x += dx * TOUCH_STEP;
            f.y += dy * TOUCH_STEP;
        });
        finger_send(env, 1);
    }
}
