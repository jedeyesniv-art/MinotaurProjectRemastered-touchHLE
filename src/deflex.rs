/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Private fork: `--deflex-pad`, controller support for Deflex that follows
//! what's on screen. The window reports every controller input
//! ([crate::window::Event::PadInput]); each press looks at the screen and
//! holds a touch on the right button for as long as it's held. The level
//! select gets a stick-driven pointer (see `Window::set_pad_cursor_enabled`).
//!
//! Screens are told apart by colour, measured on the user's screenshots
//! (fractions of a box, landscape 1024x768 points as the user sees them):
//! - pause: white letters on RESUME, RETRY and QUIT (0.11-0.14 each; at most
//!   0.025 in play)
//! - end of level: blue RETRY and QUIT buttons (0.6-0.67; 0 elsewhere)
//! - level select: blue PLAY bottom right (0.76) with no blue at QUIT
//! - play: the blue paddle buttons in both bottom corners (0.1-0.19)
//! - title: the light "deflex" logo (0.29; 0 elsewhere)

use crate::frameworks::opengles::eagl::read_screen_pixels;
use crate::frameworks::uikit::{ui_application, ui_touch};
use crate::gridrunner::{fraction, from_view_point, from_view_rect};
use crate::window::{Event, FingerId, PadInput};
use crate::Environment;
use sdl2::controller::Button;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Screen {
    Title,
    LevelSelect,
    Play,
    Pause,
    EndOfLevel,
    Other,
}

type Rect = (u32, u32, u32, u32);
const PAUSE_BUTTONS: [Rect; 3] = [(425, 270, 185, 75), (440, 505, 150, 75), (440, 622, 150, 70)];
const BOTTOM_LEFT: Rect = (20, 690, 150, 65);
const BOTTOM_MIDDLE: Rect = (440, 690, 150, 65);
const BOTTOM_RIGHT: Rect = (860, 690, 150, 65);
const CORNER_LEFT: Rect = (15, 637, 160, 118);
const CORNER_RIGHT: Rect = (850, 637, 160, 118);
const LOGO: Rect = (415, 195, 200, 65);

// Where to tap (landscape points).
const PAUSE_ICON: (f32, f32) = (512.0, 85.0);
const RESUME: (f32, f32) = (512.0, 307.0);
const PAUSE_RETRY: (f32, f32) = (512.0, 543.0);
const PAUSE_QUIT: (f32, f32) = (512.0, 657.0);
const LEFT_PADDLE: (f32, f32) = (95.0, 696.0);
const RIGHT_PADDLE: (f32, f32) = (930.0, 696.0);
const END_RETRY: (f32, f32) = (94.0, 720.0);
const END_QUIT: (f32, f32) = (512.0, 720.0);
const END_NEXT_OR_PLAY: (f32, f32) = (933.0, 720.0);
const TITLE_TAP: (f32, f32) = (512.0, 384.0);

fn score(env: &mut Environment, rect: Rect, test: impl Fn(u8, u8, u8) -> bool) -> f32 {
    let rect = from_view_rect(env, rect);
    read_screen_pixels(env, rect).map_or(0.0, |p| fraction(&p, test))
}

fn recognise(env: &mut Environment) -> Screen {
    let white = |r: u8, g: u8, b: u8| r > 215 && g > 215 && b > 215;
    let blue = |r: u8, g: u8, b: u8| b > 170 && r < 130 && g < 90;
    let light = |r: u8, g: u8, b: u8| r > 180 && g > 165 && b > 185;
    if PAUSE_BUTTONS.iter().all(|&rect| score(env, rect, white) > 0.06) {
        return Screen::Pause;
    }
    let (left, middle, _) = (
        score(env, BOTTOM_LEFT, blue),
        score(env, BOTTOM_MIDDLE, blue),
        score(env, BOTTOM_RIGHT, blue),
    );
    if left > 0.4 && middle > 0.3 {
        return Screen::EndOfLevel;
    }
    // PLAY pulses: on its dark frames only about half of it passes `blue`,
    // so it gets a looser test (0.94 on the user's screenshot, 0.66 strict).
    let play_blue = |r: u8, g: u8, b: u8| b > 110 && b > r.saturating_add(50) && b > g.saturating_add(70);
    if score(env, BOTTOM_RIGHT, play_blue) > 0.5 && left < 0.05 && middle < 0.05 {
        return Screen::LevelSelect;
    }
    // The paddle buttons are see-through, so a bright field tints them (on
    // green, 0.07-0.08 pass `blue`): a looser blue-over-the-rest test.
    let paddle_blue = |r: u8, g: u8, b: u8| b > 150 && b > g.saturating_add(60) && b > r.saturating_add(60);
    if score(env, CORNER_LEFT, paddle_blue) > 0.06 && score(env, CORNER_RIGHT, paddle_blue) > 0.06 {
        return Screen::Play;
    }
    if score(env, LOGO, light) > 0.12 {
        return Screen::Title;
    }
    Screen::Other
}

/// [recognise], but a screen recognised in the last [SCREEN_HOLD] stays
/// that screen while the check sees nothing it knows, until another screen
/// is recognised. Play stays play however long that takes: a pressed paddle
/// flashes red, and every way out of play (pause, end of level) is
/// recognised. The game strobes and pulses (PLAY, the field behind the
/// paddle buttons), so one frame that fails its colour test can't drop the
/// pointer mid-drag, stop Back quitting or lose a paddle press.
fn recognise_held(env: &mut Environment) -> Screen {
    let screen = recognise(env);
    let now = std::time::Instant::now();
    LAST_SCREEN.with(|last| {
        let mut last = last.borrow_mut();
        match (screen, *last) {
            (Screen::Other, Some((Screen::Play, _))) => Screen::Play,
            (Screen::Other, Some((held, t))) if now - t < SCREEN_HOLD => held,
            (Screen::Other, _) => {
                *last = None;
                screen
            }
            _ => {
                *last = Some((screen, now));
                screen
            }
        }
    })
}

const SCREEN_HOLD: std::time::Duration = std::time::Duration::from_millis(1500);

thread_local! {
    static LAST_SCREEN: std::cell::RefCell<Option<(Screen, std::time::Instant)>> = const { std::cell::RefCell::new(None) };
}

enum Action {
    Tap((f32, f32)),
    Quit,
    Nothing,
}

fn action(screen: Screen, input: PadInput) -> Action {
    use PadInput as P;
    // Not the stick: mashing it as a level ended tapped NEXT on the end
    // screen while the held screen still said play.
    let left_paddle = matches!(
        input,
        P::Button(Button::X | Button::DPadLeft | Button::LeftShoulder | Button::LeftStick)
            | P::LeftTrigger
    );
    let right_paddle = matches!(
        input,
        P::Button(Button::B | Button::DPadRight | Button::RightShoulder | Button::RightStick)
            | P::RightTrigger
    );
    let button = match input {
        P::Button(button) => Some(button),
        _ => None,
    };
    match (screen, button) {
        (Screen::Title, Some(Button::A)) => Action::Tap(TITLE_TAP),
        (Screen::Title | Screen::LevelSelect, Some(Button::Back)) => Action::Quit,
        (Screen::LevelSelect, Some(Button::X)) => Action::Tap(END_NEXT_OR_PLAY),
        (Screen::Play, Some(Button::Start)) => Action::Tap(PAUSE_ICON),
        (Screen::Play, _) if left_paddle => Action::Tap(LEFT_PADDLE),
        (Screen::Play, _) if right_paddle => Action::Tap(RIGHT_PADDLE),
        (Screen::Pause, Some(Button::Start)) => Action::Tap(RESUME),
        (Screen::Pause, Some(Button::X)) => Action::Tap(PAUSE_RETRY),
        (Screen::Pause, Some(Button::Back)) => Action::Tap(PAUSE_QUIT),
        (Screen::EndOfLevel, Some(Button::X)) => Action::Tap(END_RETRY),
        (Screen::EndOfLevel, Some(Button::Back)) => Action::Tap(END_QUIT),
        (Screen::EndOfLevel, Some(Button::A)) => Action::Tap(END_NEXT_OR_PLAY),
        _ => Action::Nothing,
    }
}

thread_local! {
    /// Touches held by inputs, until they're released.
    static HELD: RefCell<HashMap<PadInput, (FingerId, (f32, f32))>> = RefCell::new(HashMap::new());
    static FRAMES: RefCell<u32> = const { RefCell::new(0) };
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

fn release(env: &mut Environment, input: PadInput) {
    if let Some((finger, at)) = HELD.with(|h| h.borrow_mut().remove(&input)) {
        ui_touch::handle_event(env, Event::TouchesUp(HashMap::from([(finger, at)])));
    }
}

pub fn handle_input(env: &mut Environment, input: PadInput, down: bool) {
    // A second press of a held input (the window forgets the stick's
    // direction when the pointer comes and goes) lifts the old touch first:
    // UIKit asserts on a finger going down twice while another is held.
    release(env, input);
    if !down {
        return;
    }
    let screen = recognise_held(env);
    log!("Deflex pad: {:?} on {:?}", input, screen);
    match action(screen, input) {
        Action::Tap(point) => {
            let at = from_view_point(env, point);
            let finger = finger_for(input);
            HELD.with(|h| h.borrow_mut().insert(input, (finger, at)));
            ui_touch::handle_event(env, Event::TouchesDown(HashMap::from([(finger, at)])));
        }
        Action::Quit => {
            echo!("B pressed on Deflex's title or level select, exiting.");
            ui_application::exit(env);
        }
        Action::Nothing => (),
    }
}

/// Called each frame: every few frames, show the pointer if the level
/// select is on screen.
pub fn frame(env: &mut Environment) {
    let n = FRAMES.with(|f| {
        let mut f = f.borrow_mut();
        *f += 1;
        *f
    });
    if n % 10 != 0 {
        return;
    }
    let on_level_select = recognise_held(env) == Screen::LevelSelect;
    if on_level_select {
        // The pointer takes the stick over, so the stick's paddle touch
        // would never be lifted.
        release(env, PadInput::StickLeft);
        release(env, PadInput::StickRight);
    }
    env.window_mut().set_pad_cursor_enabled(on_level_select);
}
