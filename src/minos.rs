/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Private fork: `--minos-pad`, controller support for Caverns of Minos that
//! follows what's on screen. In play the left stick is the game's own iCade
//! joystick (`--stick-to-icade`); on the title, menu, info pages and pause
//! it moves a pointer instead, and A touches. The window reports every other
//! controller button ([crate::window::Event::PadInput]), and each press looks
//! at the screen and either holds a touch on the right button for as long as
//! it's held, holds the iCade button, or quits. In play the right trigger
//! thrusts too, in pulses that get longer the further it's pulled (see
//! [frame]).
//!
//! Screens are told apart by colour, measured on the user's screenshots and
//! our own captures (fractions of a box, portrait 768x1024 points):
//! - pause: white letters on UNPAWS and QUIT (0.27 and 0.23; at most 0.06
//!   elsewhere)
//! - title: the red bar (1.0; at most 0.03 elsewhere) on purple ground in
//!   both bottom corners (0.73-0.78)
//! - menu: blue difficulty - and NEW GAME buttons (0.83 and 0.73; 0 elsewhere)
//! - info pages: blue DONE and MORE buttons (0.74; the menu has blue there
//!   too, so it's checked first)
//! - play: the green "Shield:" and "Fuel:" labels (0.30 each). Not the bars
//!   beside them: those turn orange and olive as shield and fuel run down
//!   (0 green on the user's screenshot), which lost pause and RT thrust.
//!   Checked after pause, which shows them too.

use crate::frameworks::opengles::eagl::read_screen_pixels;
use crate::frameworks::uikit::{handle_text_input, ui_application, ui_touch};
use crate::gridrunner::fraction;
use crate::window::{Event, FingerId, PadInput, TextInputEvent};
use crate::Environment;
use sdl2::controller::Button;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Screen {
    Title,
    Menu,
    Info,
    Play,
    Pause,
    Other,
}

type Rect = (u32, u32, u32, u32);
const UNPAWS: Rect = (200, 370, 370, 120);
const QUIT: Rect = (250, 785, 270, 120);
const TITLE_RED: Rect = (250, 812, 260, 25);
const TITLE_GROUND: [Rect; 2] = [(40, 880, 120, 40), (610, 880, 120, 40)];
const MENU_DIFFICULTY_MINUS: Rect = (5, 520, 60, 90);
const MENU_NEW_GAME: Rect = (5, 770, 60, 80);
const BOTTOM_LEFT: Rect = (5, 945, 60, 70);
const BOTTOM_RIGHT: Rect = (703, 945, 60, 70);
const SHIELD_LABEL: Rect = (78, 68, 232, 26);
const FUEL_LABEL: Rect = (78, 103, 155, 26);

// Where to tap (portrait points).
const PAWS_TAP: (f32, f32) = (385.0, 597.0);
const UNPAWS_TAP: (f32, f32) = (384.0, 428.0);
const QUIT_TAP: (f32, f32) = (384.0, 846.0);
const DIFFICULTY_MINUS_TAP: (f32, f32) = (60.0, 567.0);
const DIFFICULTY_PLUS_TAP: (f32, f32) = (708.0, 567.0);
const INFO_OR_DONE_TAP: (f32, f32) = (90.0, 978.0);
const MORE_TAP: (f32, f32) = (678.0, 978.0);

/// iCade button A's letters (pressed, released).
const ICADE_A: (&str, &str) = ("y", "t");

/// The right trigger's thrust: below `TRIGGER_OFF` it's off, above
/// `TRIGGER_FULL` it's held, and in between it's held for 1 to
/// `PULSE_FRAMES` frames out of every `PULSE_FRAMES`, in proportion.
const TRIGGER_OFF: f32 = 0.08;
const TRIGGER_FULL: f32 = 0.92;
const PULSE_FRAMES: u32 = 8;

fn score(env: &mut Environment, rect: Rect, test: impl Fn(u8, u8, u8) -> bool) -> f32 {
    read_screen_pixels(env, rect).map_or(0.0, |p| fraction(&p, test))
}

fn recognise(env: &mut Environment) -> Screen {
    let white = |r: u8, g: u8, b: u8| r > 190 && g > 190 && b > 190;
    let red = |r: u8, g: u8, b: u8| r > 200 && g < 60 && b < 60;
    let purple = |r: u8, g: u8, b: u8| {
        (120..=200).contains(&r) && (50..=110).contains(&g) && (140..=220).contains(&b) && b > g + 50
    };
    let blue = |r: u8, g: u8, b: u8| b > 200 && r < 90 && g < 90;
    let green = |r: u8, g: u8, b: u8| g > 150 && r < 120 && b < 120;
    if score(env, UNPAWS, white) > 0.15 && score(env, QUIT, white) > 0.12 {
        return Screen::Pause;
    }
    if score(env, TITLE_RED, red) > 0.5 && TITLE_GROUND.iter().all(|&rect| score(env, rect, purple) > 0.4) {
        return Screen::Title;
    }
    let corners = score(env, BOTTOM_LEFT, blue) > 0.4 && score(env, BOTTOM_RIGHT, blue) > 0.4;
    if score(env, MENU_DIFFICULTY_MINUS, blue) > 0.4 && score(env, MENU_NEW_GAME, blue) > 0.4 {
        return Screen::Menu;
    }
    if corners {
        return Screen::Info;
    }
    if score(env, SHIELD_LABEL, green) > 0.15 && score(env, FUEL_LABEL, green) > 0.15 {
        return Screen::Play;
    }
    Screen::Other
}

enum Action {
    Tap((f32, f32)),
    /// Hold the game's iCade button.
    ICade,
    Quit,
    Nothing,
}

fn action(screen: Screen, button: Button) -> Action {
    // A only gets here where there's no pointer (play and the in-between
    // screens); Start does what A did on the pointer's screens.
    match (screen, button) {
        (Screen::Info, Button::Start) => Action::Tap(MORE_TAP),
        (Screen::Title | Screen::Menu, Button::Start) => Action::ICade,
        (_, Button::A) => Action::ICade,
        (Screen::Title | Screen::Menu, Button::Back) => Action::Quit,
        (Screen::Info, Button::B | Button::Back) => Action::Tap(INFO_OR_DONE_TAP),
        (Screen::Pause, Button::Back) => Action::Tap(QUIT_TAP),
        (Screen::Play, Button::Start) => Action::Tap(PAWS_TAP),
        (Screen::Pause, Button::Start) => Action::Tap(UNPAWS_TAP),
        (Screen::Menu, Button::X) => Action::Tap(INFO_OR_DONE_TAP),
        (Screen::Menu, Button::DPadLeft) => Action::Tap(DIFFICULTY_MINUS_TAP),
        (Screen::Menu, Button::DPadRight) => Action::Tap(DIFFICULTY_PLUS_TAP),
        _ => Action::Nothing,
    }
}

/// What a held button is holding, to let go of when it's released.
enum Held {
    Touch(FingerId, (f32, f32)),
    ICade,
}

#[derive(Default)]
struct Thrust {
    /// A holds the iCade button.
    by_a: bool,
    /// The right trigger holds the iCade button.
    by_trigger: bool,
    /// The trigger is pulled past `TRIGGER_OFF`.
    pulled: bool,
    /// The pull started in play, so it thrusts until released.
    in_play: bool,
    /// Frame within the current pulse.
    frame: u32,
}

/// How often (in frames) to check the screen for the pointer and the bars.
const BARS_CHECK_FRAMES: u32 = 10;

thread_local! {
    static HELD: RefCell<HashMap<Button, Held>> = RefCell::new(HashMap::new());
    static THRUST: RefCell<Thrust> = RefCell::new(Thrust::default());
    static FRAMES: RefCell<u32> = const { RefCell::new(0) };
}

/// Holds or lets go of the iCade button for A or the trigger, telling the
/// game only when the two together change.
fn set_icade(env: &mut Environment, update: impl FnOnce(&mut Thrust)) {
    let (before, after) = THRUST.with(|t| {
        let mut t = t.borrow_mut();
        let before = t.by_a || t.by_trigger;
        update(&mut t);
        (before, t.by_a || t.by_trigger)
    });
    if before != after {
        let letter = if after { ICADE_A.0 } else { ICADE_A.1 };
        handle_text_input(env, TextInputEvent::Text(letter.to_string()));
    }
}

/// Called each frame: every few frames the pointer (on the title, menu, info
/// pages and pause) and the bars (the `--ambient-bars` glow on the menu and
/// info pages, whose blue buttons would make odd `--edge-bars`, and the
/// cavern walls carried outwards everywhere else); and the right trigger's
/// thrust.
pub fn frame(env: &mut Environment) {
    let n = FRAMES.with(|f| {
        let mut f = f.borrow_mut();
        *f = f.wrapping_add(1);
        *f
    });
    if n % BARS_CHECK_FRAMES == 0 {
        let screen = recognise(env);
        let pointer = matches!(screen, Screen::Title | Screen::Menu | Screen::Info | Screen::Pause);
        env.window_mut().set_pad_cursor_enabled(pointer);
        if env.options.edge_bars.is_some() {
            let glow = matches!(screen, Screen::Menu | Screen::Info);
            env.window_mut().set_edge_bars_off(glow);
        }
    }
    let pull = env.window().right_trigger();
    let pulled = THRUST.with(|t| t.borrow().pulled);
    if pull < TRIGGER_OFF {
        if pulled {
            THRUST.with(|t| t.borrow_mut().pulled = false);
            set_icade(env, |t| t.by_trigger = false);
        }
        return;
    }
    if !pulled {
        let screen = recognise(env);
        log!("Minos pad: right trigger on {:?}", screen);
        THRUST.with(|t| {
            let mut t = t.borrow_mut();
            t.pulled = true;
            t.in_play = screen == Screen::Play;
            t.frame = 0;
        });
    }
    let (in_play, frame) = THRUST.with(|t| {
        let t = t.borrow();
        (t.in_play, t.frame)
    });
    if !in_play {
        return;
    }
    let share = ((pull - TRIGGER_OFF) / (TRIGGER_FULL - TRIGGER_OFF)).min(1.0);
    let on_frames = ((share * PULSE_FRAMES as f32).ceil() as u32).clamp(1, PULSE_FRAMES);
    THRUST.with(|t| t.borrow_mut().frame = (frame + 1) % PULSE_FRAMES);
    set_icade(env, |t| t.by_trigger = frame < on_frames);
}

pub fn handle_input(env: &mut Environment, input: PadInput, down: bool) {
    let PadInput::Button(button) = input else {
        return;
    };
    if !down {
        match HELD.with(|h| h.borrow_mut().remove(&button)) {
            Some(Held::Touch(finger, at)) => {
                ui_touch::handle_event(env, Event::TouchesUp(HashMap::from([(finger, at)])));
            }
            Some(Held::ICade) => set_icade(env, |t| t.by_a = false),
            None => (),
        }
        return;
    }
    if HELD.with(|h| h.borrow().contains_key(&button)) {
        return;
    }
    let screen = recognise(env);
    log!("Minos pad: {:?} on {:?}", button, screen);
    if matches!((&screen, button), (Screen::Pause, Button::Back)) {
        crate::scores::post_running_score(env);
    }
    match action(screen, button) {
        Action::Tap(at) => {
            let finger = FingerId::Pad(button as u8);
            HELD.with(|h| h.borrow_mut().insert(button, Held::Touch(finger, at)));
            ui_touch::handle_event(env, Event::TouchesDown(HashMap::from([(finger, at)])));
        }
        Action::ICade => {
            HELD.with(|h| h.borrow_mut().insert(button, Held::ICade));
            set_icade(env, |t| t.by_a = true);
        }
        Action::Quit => {
            echo!("B pressed on Caverns of Minos' title or menu, exiting.");
            ui_application::exit(env);
        }
        Action::Nothing => (),
    }
}
