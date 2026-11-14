/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Private fork: console-style menus for the pad modules. A pad module gives
//! the buttons of the screen it recognises (rectangles in the app's screen
//! points as the user sees them); the stick and d-pad move a red highlight
//! between them (to the nearest button in that direction), and A taps the
//! highlighted one, held for as long as A is. The window turns the stick's
//! pushes into moves while focus is on (`Window::set_menu_focus`), and the
//! presenter draws the highlight (`Window::menu_focus_screen_rect`).

use crate::frameworks::uikit::ui_touch;
use crate::gridrunner::from_view_point;
use crate::window::{Event, FingerId};
use crate::Environment;
use sdl2::controller::Button;
use std::cell::RefCell;
use std::collections::HashMap;

/// A button: x, y, width, height.
pub type Rect = [f32; 4];

#[derive(Default)]
struct State {
    /// Which screen the buttons are for (the pad module's own number).
    screen: Option<u32>,
    buttons: Vec<Rect>,
    focus: usize,
    /// Where A is touching, while it's held.
    touching: Option<(f32, f32)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

const FINGER: FingerId = FingerId::Pad(250);

fn centre([x, y, w, h]: Rect) -> (f32, f32) {
    (x + w / 2.0, y + h / 2.0)
}

/// Show these buttons for this screen (`None`: no focus here). A new screen
/// starts on its `first` button; the same screen keeps its focus.
pub fn set(env: &mut Environment, screen: Option<(u32, &[Rect], usize)>) {
    let Some((id, buttons, first)) = screen else {
        let was = STATE.with(|s| s.borrow_mut().screen.take());
        if was.is_some() {
            lift(env);
            env.window_mut().set_menu_focus(false);
        }
        return;
    };
    let changed = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let changed = s.screen != Some(id);
        if changed {
            s.screen = Some(id);
            s.focus = first.min(buttons.len().saturating_sub(1));
        }
        s.buttons = buttons.to_vec();
        changed
    });
    if changed {
        log!("Menu focus: screen {} ({} buttons)", id, buttons.len());
    }
    let window = env.window_mut();
    window.set_menu_focus(true);
    window.set_pad_cursor_enabled(false);
    update_rect(env);
}

/// The focus's screen, if it's on.
pub fn screen() -> Option<u32> {
    STATE.with(|s| s.borrow().screen)
}

fn update_rect(env: &mut Environment) {
    let rect = STATE.with(|s| {
        let s = s.borrow();
        s.screen.and_then(|_| s.buttons.get(s.focus).copied())
    });
    env.window_mut().set_menu_focus_rect(rect);
}

/// Move the highlight to the nearest button in a direction (x, y: -1, 0, 1),
/// within a wide cone; nowhere to go leaves it where it is.
pub fn navigate(env: &mut Environment, (dx, dy): (i8, i8)) {
    let (dx, dy) = (dx as f32, dy as f32);
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(&from) = s.buttons.get(s.focus) else { return };
        let (cx, cy) = centre(from);
        let best = s
            .buttons
            .iter()
            .enumerate()
            .filter_map(|(i, &b)| {
                let (px, py) = centre(b);
                let along = (px - cx) * dx + (py - cy) * dy;
                let across = ((px - cx) * dy - (py - cy) * dx).abs();
                (along > 1.0 && across <= along * 3.0).then_some((i, along + 2.0 * across))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _)) = best {
            s.focus = i;
        }
    });
    update_rect(env);
}

/// Each frame: the stick's moves.
pub fn frame(env: &mut Environment) {
    if screen().is_none() {
        return;
    }
    for step in env.window_mut().take_menu_nav() {
        navigate(env, step);
    }
}

/// A d-pad direction, if the button is one.
pub fn dpad(button: Button) -> Option<(i8, i8)> {
    match button {
        Button::DPadUp => Some((0, -1)),
        Button::DPadDown => Some((0, 1)),
        Button::DPadLeft => Some((-1, 0)),
        Button::DPadRight => Some((1, 0)),
        _ => None,
    }
}

/// A pressed or let go while focus is on: touch the highlighted button.
/// Returns whether focus took it (d-pad presses move the highlight too).
pub fn handle_button(env: &mut Environment, button: Button, down: bool) -> bool {
    if screen().is_none() {
        // Let go of a touch the focus started on a screen since left.
        if !down && button == Button::A && STATE.with(|s| s.borrow().touching.is_some()) {
            lift(env);
            return true;
        }
        return false;
    }
    if let Some(step) = dpad(button) {
        if down {
            navigate(env, step);
        }
        return true;
    }
    if button != Button::A {
        return false;
    }
    if !down {
        lift(env);
        return true;
    }
    let point = STATE.with(|s| {
        let s = s.borrow();
        s.buttons.get(s.focus).map(|&b| centre(b))
    });
    if let Some(point) = point {
        lift(env);
        let at = from_view_point(env, point);
        log!("Menu focus: A taps {:?}", point);
        STATE.with(|s| s.borrow_mut().touching = Some(at));
        ui_touch::handle_event(env, Event::TouchesDown(HashMap::from([(FINGER, at)])));
    }
    true
}

fn lift(env: &mut Environment) {
    if let Some(at) = STATE.with(|s| s.borrow_mut().touching.take()) {
        ui_touch::handle_event(env, Event::TouchesUp(HashMap::from([(FINGER, at)])));
    }
}
