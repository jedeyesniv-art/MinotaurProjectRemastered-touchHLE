/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Private fork: `--goat-pad`, controller support for Goat Up that follows
//! what's on screen. The game has no iCade support, so movement is its tilt
//! control type (C) on the left stick (`--left-stick-tilt`); the window
//! reports every controller button ([crate::window::Event::PadInput]), and
//! each press looks at the screen and either holds a touch on the right spot
//! for as long as it's held (so A's jump is short or long like a finger's),
//! or quits.
//!
//! Screens are told apart by colour, measured on the user's screenshots and
//! our own captures (fractions of a box, portrait 768x1024 points):
//! - pause: the blue RESUME and QUIT signs (0.64 and 0.79; 0 elsewhere)
//! - info pages: the MAIN MENU and MORE INFO buttons, by shape, since their
//!   colours flash: lit top and bottom edges (0.98-1.0; at most 0.62 top and
//!   0 bottom elsewhere) with a dark strip just inside the top (0.05)
//! - menu: the boxes' left and right borders, whose colours cycle (0.57-0.96;
//!   at most 0.10 elsewhere)
//! - anything else is play (or GOAT OVER, where a touch carries on too).

use crate::frameworks::opengles::eagl::read_screen_pixels;
use crate::frameworks::uikit::{ui_application, ui_touch};
use crate::gridrunner::fraction;
use crate::window::{Event, FingerId, PadInput};
use crate::Environment;
use sdl2::controller::Button;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Screen {
    Menu,
    Info,
    Play,
    Pause,
}

type Rect = (u32, u32, u32, u32);
const RESUME: Rect = (250, 370, 270, 100);
const QUIT: Rect = (250, 805, 270, 95);
/// The info pages' two buttons (x, width); see [is_info].
const INFO_BUTTONS: [(u32, u32); 2] = [(24, 290), (556, 186)];
const MENU_LEFT: Rect = (24, 360, 12, 630);
const MENU_RIGHT: Rect = (734, 360, 10, 630);

// Where to tap (portrait points).
const START_TAP: (f32, f32) = (384.0, 875.0);
const CONTROL_TYPE_TAP: (f32, f32) = (384.0, 643.0);
const MENU_MORE_INFO_TAP: (f32, f32) = (200.0, 440.0);
const INFO_MAIN_MENU_TAP: (f32, f32) = (170.0, 962.0);
const INFO_MORE_INFO_TAP: (f32, f32) = (648.0, 962.0);
const PAUSE_TAP: (f32, f32) = (384.0, 185.0);
const RESUME_TAP: (f32, f32) = (384.0, 420.0);
const QUIT_TAP: (f32, f32) = (384.0, 853.0);
/// Jump: any touch jumps in control type C; the bottom right corner is also
/// the jump side in type A.
const JUMP_TAP: (f32, f32) = (690.0, 960.0);

fn score(env: &mut Environment, rect: Rect, test: impl Fn(u8, u8, u8) -> bool) -> f32 {
    read_screen_pixels(env, rect).map_or(0.0, |p| fraction(&p, test))
}

/// Both info buttons' outlines: lit top and bottom edges with a dark strip
/// inside the top one, in any colour.
fn is_info(env: &mut Environment) -> bool {
    let lit = |r: u8, g: u8, b: u8| r.max(g).max(b) > 80;
    INFO_BUTTONS.iter().all(|&(x, w)| {
        score(env, (x, 924, w, 8), lit) > 0.75
            && score(env, (x, 992, w, 8), lit) > 0.75
            && score(env, (x, 938, w, 12), lit) < 0.2
    })
}

fn recognise(env: &mut Environment) -> Screen {
    let blue = |r: u8, g: u8, b: u8| b > 170 && r < 110 && g < 110;
    let bright = |r: u8, g: u8, b: u8| r.max(g).max(b) > 120;
    if score(env, RESUME, blue) > 0.3 && score(env, QUIT, blue) > 0.3 {
        return Screen::Pause;
    }
    if is_info(env) {
        return Screen::Info;
    }
    if score(env, MENU_LEFT, bright) > 0.35 && score(env, MENU_RIGHT, bright) > 0.35 {
        return Screen::Menu;
    }
    Screen::Play
}

enum Action {
    Tap((f32, f32)),
    Quit,
    Nothing,
}

fn action(screen: Screen, button: Button) -> Action {
    match (screen, button) {
        (Screen::Menu, Button::A) => Action::Tap(START_TAP),
        (Screen::Menu, Button::X) => Action::Tap(CONTROL_TYPE_TAP),
        (Screen::Menu, Button::Y) => Action::Tap(MENU_MORE_INFO_TAP),
        (Screen::Menu, Button::B) => Action::Quit,
        (Screen::Info, Button::A) => Action::Tap(INFO_MORE_INFO_TAP),
        (Screen::Info, Button::B) => Action::Tap(INFO_MAIN_MENU_TAP),
        (Screen::Play, Button::A) => Action::Tap(JUMP_TAP),
        (Screen::Play, Button::Start) => Action::Tap(PAUSE_TAP),
        (Screen::Pause, Button::Start) => Action::Tap(RESUME_TAP),
        (Screen::Pause, Button::B) => Action::Tap(QUIT_TAP),
        _ => Action::Nothing,
    }
}

thread_local! {
    /// The touch each held button is holding, to let go of when it's released.
    static HELD: RefCell<HashMap<Button, (FingerId, (f32, f32))>> = RefCell::new(HashMap::new());
}

pub fn handle_input(env: &mut Environment, input: PadInput, down: bool) {
    let PadInput::Button(button) = input else {
        return;
    };
    if !down {
        if let Some((finger, at)) = HELD.with(|h| h.borrow_mut().remove(&button)) {
            ui_touch::handle_event(env, Event::TouchesUp(HashMap::from([(finger, at)])));
        }
        return;
    }
    if HELD.with(|h| h.borrow().contains_key(&button)) {
        return;
    }
    let screen = recognise(env);
    log!("Goat pad: {:?} on {:?}", button, screen);
    match action(screen, button) {
        Action::Tap(at) => {
            let finger = FingerId::Pad(button as u8);
            HELD.with(|h| h.borrow_mut().insert(button, (finger, at)));
            ui_touch::handle_event(env, Event::TouchesDown(HashMap::from([(finger, at)])));
        }
        Action::Quit => {
            echo!("B pressed on Goat Up's menu, exiting.");
            ui_application::exit(env);
        }
        Action::Nothing => (),
    }
}
