/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Private fork: `--goat2-pad`, controller support for Goat Up 2 that follows
//! what's on screen. Play is the tilt control type on the left stick
//! (`--left-stick-tilt`), with A holding a touch to jump. The menus (game and
//! level select, the editor) have many buttons, so they get the stick-driven
//! pointer (`Window::set_pad_cursor_enabled`; the window gives A to it while
//! it's shown). Other buttons are reported as [crate::window::Event::PadInput]
//! and each press looks at the screen and holds a touch on the right spot for
//! as long as it's held, or quits.
//!
//! Screens are told apart by colour, measured on the user's screenshots and
//! our own captures (fractions of a box, portrait 768x1024 points):
//! - pause: the Exit sign, a dark grey face (55,55,55) with light grey
//!   letters and border (at least 0.60 and 0.15 on 21 paused frames; at most
//!   0.013 and 0.004 on 84 others). RESET blinks every quarter second, so it
//!   can't be used.
//! - level select: the green PLAY button (0.77; at most 0.10 elsewhere)
//! - game select (play or edit mode): the blue MODE button top right (0.57;
//!   0.43 on the level select's BACK, so PLAY is checked first)
//! - play: the red STATUS label bottom left (0.62; 0 elsewhere). The attract
//!   demo shows it too, where A's touch begins the game like any touch.
//! - title: the bright green BUCK! sign (0.22; at most 0.016 elsewhere), or
//!   the attract demo's TOUCH TO BEGIN strip, by its border's right edge
//!   (0.09-0.13; 0 elsewhere)
//! - anything else (the editor, end of level, ...) gets the pointer.

use crate::abi::DotDotDot;
use crate::frameworks::opengles::eagl::read_screen_pixels;
use crate::frameworks::uikit::{ui_application, ui_touch};
use crate::gridrunner::fraction;
use crate::libc::stdio::printf::printf_inner;
use crate::mem::{ConstPtr, MutPtr, Ptr};
use crate::window::{Event, FingerId, PadInput};
use crate::Environment;
use sdl2::controller::Button;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Screen {
    Title,
    GameSelect,
    LevelSelect,
    Play,
    Pause,
    Other,
}

type Rect = (u32, u32, u32, u32);
const EXIT: Rect = (280, 585, 210, 110);
const PLAY_BUTTON: Rect = (290, 860, 190, 120);
const MODE_BUTTON: Rect = (695, 55, 45, 60);
const STATUS: Rect = (5, 940, 140, 25);
const BUCK: Rect = (60, 60, 560, 450);
const STRIP_EDGE: Rect = (750, 905, 14, 70);

// Where to tap (portrait points).
const TITLE_TAP: (f32, f32) = (384.0, 700.0);
/// The pause icon (a faint heart in play) and the unpause one are the same.
const PAUSE_TAP: (f32, f32) = (384.0, 150.0);
const RESET_TAP: (f32, f32) = (384.0, 410.0);
const EXIT_TAP: (f32, f32) = (384.0, 638.0);
const CONTROL_TYPE_TAP: (f32, f32) = (384.0, 840.0);
const BACK_TAP: (f32, f32) = (717.0, 97.0);
/// Jump: any touch jumps in the tilt control type; the bottom right is also
/// the jump side in the touch types.
const JUMP_TAP: (f32, f32) = (690.0, 960.0);

fn score(env: &mut Environment, rect: Rect, test: impl Fn(u8, u8, u8) -> bool) -> f32 {
    read_screen_pixels(env, rect).map_or(0.0, |p| fraction(&p, test))
}

fn recognise(env: &mut Environment) -> Screen {
    let neutral = |r: u8, g: u8, b: u8| r.abs_diff(g) < 10 && g.abs_diff(b) < 10;
    let dark_grey = |r: u8, g: u8, b: u8| neutral(r, g, b) && (40..=70).contains(&r);
    let light_grey = |r: u8, g: u8, b: u8| neutral(r, g, b) && (125..=175).contains(&r);
    let green = |r: u8, g: u8, b: u8| g > 140 && r < 90 && b < 90;
    let blue = |r: u8, _g: u8, b: u8| b > 140 && r < 110;
    let red = |r: u8, g: u8, b: u8| r > 90 && g < 50 && b < 50;
    let bright_green = |r: u8, g: u8, b: u8| g > 180 && r < 90 && b < 90;
    let lit = |r: u8, g: u8, b: u8| r.max(g).max(b) > 100;
    if score(env, EXIT, dark_grey) > 0.3 && score(env, EXIT, light_grey) > 0.06 {
        return Screen::Pause;
    }
    if score(env, PLAY_BUTTON, green) > 0.4 {
        return Screen::LevelSelect;
    }
    if score(env, MODE_BUTTON, blue) > 0.3 {
        return Screen::GameSelect;
    }
    if score(env, STATUS, red) > 0.3 {
        return Screen::Play;
    }
    if score(env, BUCK, bright_green) > 0.1 || score(env, STRIP_EDGE, lit) > 0.05 {
        return Screen::Title;
    }
    Screen::Other
}

fn wants_pointer(screen: Screen) -> bool {
    matches!(screen, Screen::GameSelect | Screen::LevelSelect | Screen::Other)
}

enum Action {
    Tap((f32, f32)),
    Quit,
    Nothing,
}

fn action(screen: Screen, button: Button) -> Action {
    match (screen, button) {
        (Screen::Title, Button::A) => Action::Tap(TITLE_TAP),
        (Screen::Title | Screen::GameSelect, Button::Back) => Action::Quit,
        (Screen::LevelSelect, Button::B | Button::Back) => Action::Tap(BACK_TAP),
        (Screen::Play, Button::A) => Action::Tap(JUMP_TAP),
        (Screen::Play | Screen::Pause, Button::Start) => Action::Tap(PAUSE_TAP),
        (Screen::Pause, Button::Y) => Action::Tap(RESET_TAP),
        (Screen::Pause, Button::Back) => Action::Tap(EXIT_TAP),
        (Screen::Pause, Button::X) => Action::Tap(CONTROL_TYPE_TAP),
        _ => Action::Nothing,
    }
}

thread_local! {
    /// The touch each held button is holding, to let go of when it's released.
    static HELD: RefCell<HashMap<Button, (FingerId, (f32, f32))>> = RefCell::new(HashMap::new());
    static FRAMES: RefCell<u32> = const { RefCell::new(0) };
}

fn release(env: &mut Environment, button: Button) {
    if let Some((finger, at)) = HELD.with(|h| h.borrow_mut().remove(&button)) {
        ui_touch::handle_event(env, Event::TouchesUp(HashMap::from([(finger, at)])));
    }
}

pub fn handle_input(env: &mut Environment, input: PadInput, down: bool) {
    let PadInput::Button(button) = input else {
        return;
    };
    if !down {
        release(env, button);
        return;
    }
    if HELD.with(|h| h.borrow().contains_key(&button)) {
        return;
    }
    let screen = recognise(env);
    log!("Goat Up 2 pad: {:?} on {:?}", button, screen);
    match action(screen, button) {
        Action::Tap(at) => {
            let finger = FingerId::Pad(button as u8);
            HELD.with(|h| h.borrow_mut().insert(button, (finger, at)));
            ui_touch::handle_event(env, Event::TouchesDown(HashMap::from([(finger, at)])));
        }
        Action::Quit => {
            echo!("B pressed on Goat Up 2's title or game select, exiting.");
            ui_application::exit(env);
        }
        Action::Nothing => (),
    }
}

/// Called each frame: every few frames, show the pointer on the menus. A
/// held A (a jump) is let go first, since while the pointer is shown A's
/// release goes to the pointer.
pub fn frame(env: &mut Environment) {
    let n = FRAMES.with(|f| {
        let mut f = f.borrow_mut();
        *f += 1;
        *f
    });
    if n % 10 != 0 {
        return;
    }
    let screen = recognise(env);
    let pointer = wants_pointer(screen);
    if pointer {
        let held: Vec<Button> = HELD.with(|h| h.borrow().keys().copied().collect());
        for button in held {
            release(env, button);
        }
    }
    env.window_mut().set_pad_cursor_enabled(pointer);
}

/// The engine's debug log functions (printf-style, compiled to empty
/// stubs in the release build: `push {r1, r2, r3}; add sp, #0xc; bx lr`).
const LOG_FUNCTIONS: [u32; 4] = [0x3ce54, 0x3ce5c, 0x3ce64, 0x3ce6c];

/// With the `TOUCHHLE_GAME_LOG` environment variable set, point the engine's
/// empty debug log functions at [game_log], so its own messages ("Loading
/// Module.", "MSTO", ...) appear as `GAMELOG` lines. It showed where start-up
/// stalled when `rand()` didn't match iOS. For debugging only: some messages
/// may use formats touchHLE's printf doesn't support.
pub fn apply_log_patch(env: &mut Environment) {
    if env.bundle.bundle_identifier() != "co.uk.llamasoft.Grunt"
        || std::env::var_os("TOUCHHLE_GAME_LOG").is_none()
    {
        return;
    }
    if LOG_FUNCTIONS
        .iter()
        .any(|&at| env.mem.read(Ptr::<u16, false>::from_bits(at)) != 0xb40e)
    {
        log!("TOUCHHLE_GAME_LOG: unexpected code, is this Goat Up 2 1.0? Not patching.");
        return;
    }
    let host = env.dyld.create_guest_function(
        &mut env.mem,
        "_touchHLE_goat2_game_log",
        &(game_log as fn(&mut Environment, ConstPtr<u8>, DotDotDot)),
    );
    for at in LOG_FUNCTIONS {
        // Thumb-2: ldr.w pc, [pc, #0] (to the A32 host stub), then the literal.
        let code: MutPtr<u16> = Ptr::from_bits(at);
        env.mem.write(code, 0xf8df);
        env.mem.write(code + 1, 0xf000);
        env.mem.write(Ptr::<u32, true>::from_bits(at + 4), host.addr_with_thumb_bit());
        env.cpu.invalidate_cache_range(at, 8);
    }
    log!("TOUCHHLE_GAME_LOG: printing Goat Up 2's debug log.");
}

fn game_log(env: &mut Environment, format: ConstPtr<u8>, args: DotDotDot) {
    let res = printf_inner::<false, _>(env, |mem, idx| mem.read(format + idx), args.start());
    echo!("GAMELOG {}", String::from_utf8_lossy(&res).trim_end());
}
