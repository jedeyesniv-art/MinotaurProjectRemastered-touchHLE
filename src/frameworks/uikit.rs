/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The UIKit framework.
//!
//! For the time being the focus of this project is on running games, which are
//! likely to use UIKit in very simple and limited ways, so this implementation
//! will probably take a lot of shortcuts.

use crate::{msg, Environment};
use std::time::Instant;

pub mod ui_accelerometer;
pub mod ui_activity_indicator_view;
pub mod ui_application;
pub mod ui_color;
pub mod ui_device;
pub mod ui_event;
pub mod ui_font;
pub mod ui_geometry;
pub mod ui_graphics;
pub mod ui_image;
pub mod ui_image_picker_controller;
pub mod ui_local_notification;
pub mod ui_nib;
pub mod ui_pasteboard;
pub mod ui_responder;
pub mod ui_screen;
pub mod ui_touch;
pub mod ui_view;
pub mod ui_view_controller;

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/UIKit.framework/UIKit",
    aliases: &[],
    class_exports: &[
        ui_accelerometer::CLASSES,
        ui_activity_indicator_view::CLASSES,
        ui_application::CLASSES,
        ui_color::CLASSES,
        ui_device::CLASSES,
        ui_event::CLASSES,
        ui_font::CLASSES,
        ui_image::CLASSES,
        ui_image_picker_controller::CLASSES,
        ui_local_notification::CLASSES,
        ui_nib::CLASSES,
        ui_pasteboard::CLASSES,
        ui_responder::CLASSES,
        ui_screen::CLASSES,
        ui_touch::CLASSES,
        ui_view::CLASSES,
        ui_view::ui_alert_view::CLASSES,
        ui_view::ui_control::CLASSES,
        ui_view::ui_control::ui_button::CLASSES,
        ui_view::ui_control::ui_segmented_control::CLASSES,
        ui_view::ui_control::ui_slider::CLASSES,
        ui_view::ui_control::ui_text_field::CLASSES,
        ui_view::ui_control::ui_switch::CLASSES,
        ui_view::ui_image_view::CLASSES,
        ui_view::ui_label::CLASSES,
        ui_view::ui_page_control::CLASSES,
        ui_view::ui_picker_view::CLASSES,
        ui_view::ui_scroll_view::CLASSES,
        ui_view::ui_scroll_view::ui_text_view::CLASSES,
        ui_view::ui_web_view::CLASSES,
        ui_view::ui_window::CLASSES,
        ui_view_controller::CLASSES,
        ui_view_controller::ui_navigation_controller::CLASSES,
    ],
    constant_exports: &[
        ui_application::CONSTANTS,
        ui_device::CONSTANTS,
        ui_view::ui_control::ui_text_field::CONSTANTS,
        ui_view::ui_window::CONSTANTS,
    ],
    function_exports: &[
        ui_application::FUNCTIONS,
        ui_geometry::FUNCTIONS,
        ui_graphics::FUNCTIONS,
    ],
};

#[derive(Default)]
pub struct State {
    ui_accelerometer: ui_accelerometer::State,
    ui_application: ui_application::State,
    ui_color: ui_color::State,
    ui_device: ui_device::State,
    ui_font: ui_font::State,
    ui_graphics: ui_graphics::State,
    ui_image: ui_image::State,
    ui_screen: ui_screen::State,
    ui_touch: ui_touch::State,
    pub ui_view: ui_view::State,
    ui_responder: ui_responder::State,
}

/// For use by `NSRunLoop`: handles any events that have queued up.
///
/// Returns the next time this function must be called, if any, e.g. the next
/// time an accelerometer input is due.
pub fn handle_events(env: &mut Environment) -> Option<Instant> {
    use crate::window::Event;

    // NSRunLoop will never call this function in headless mode.
    while let Some(event) = env.window_mut().pop_event() {
        match event {
            Event::Quit => {
                echo!("User requested quit, exiting.");
                ui_application::exit(env);
            }
            Event::TouchesDown(..) | Event::TouchesMove(..) | Event::TouchesUp(..) => {
                ui_touch::handle_event(env, event)
            }
            Event::AppWillResignActive => {
                // Getting this event means touchHLE is becoming inactive, e.g.
                // due to switching apps. The obvious way to handle this would
                // be to just send `applicationWillResignActive:` to the
                // UIApplicationDelegate. However:
                // - touchHLE's event loop can't handle an inactive app well
                //   right now. For example, audio isn't paused.
                // - touchHLE's event loop can't handle the subsequent
                //   termination of an app right now: it doesn't manage to send
                //   the `applicationWillTerminate:` message in time. This can
                //   mean loss of data!
                // Therefore, for the moment we will simulate the early iOS
                // behavior where switching app usually resulted in termination.
                // We can usually handle this in time, so there won't be data
                // loss, nor problems with background resource usage or audio.
                // TODO: Handle this better.
                log!("Handling app-will-resign-active event: exiting.");
                ui_application::exit(env);
            }
            Event::AppWillTerminate => {
                log!("Handling app-will-terminate event.");
                ui_application::exit(env);
            }
            Event::EnterDebugger => {
                if env.is_debugging_enabled() {
                    log!("Handling EnterDebugger event: entering debugger.");
                    env.enter_debugger(/* reason: */ None);
                } else {
                    log!("Ignoring EnterDebugger event: no debugger connected.");
                }
            }
            Event::ExitButton(down) => {
                screen_button(env, crate::gridrunner::ScreenButton::Exit, down);
            }
            Event::RetryButton(down) => {
                screen_button(env, crate::gridrunner::ScreenButton::Retry, down);
            }
            Event::PadInput(input, down) => {
                if env.options.minos_pad {
                    crate::minos::handle_input(env, input, down)
                } else if env.options.goat_pad {
                    crate::goat::handle_input(env, input, down)
                } else if env.options.goat2_pad {
                    crate::goat2::handle_input(env, input, down)
                } else if env.options.rescue_pad {
                    crate::rescue::handle_input(env, input, down)
                } else {
                    crate::deflex::handle_input(env, input, down)
                }
            }
            Event::ToggleRotation => {
                use crate::window::DeviceOrientation;
                let new_orientation = match env.window().current_rotation() {
                    DeviceOrientation::Portrait | DeviceOrientation::PortraitUpsideDown => {
                        DeviceOrientation::LandscapeLeft
                    }
                    _ => DeviceOrientation::Portrait,
                };
                log!("Turning the device to {:?}.", new_orientation);
                env.on_parent_stack_in_coroutine(|window, _| window.rotate_device(new_orientation));
                if env.options.nearest_textures && env.options.smooth_render_textures {
                    let landscape = new_orientation == DeviceOrientation::LandscapeLeft;
                    crate::frameworks::opengles::set_render_textures_crisp(env, landscape);
                }
                ui_device::generate_device_orientation_notification(env);
            }
            Event::TextInput(text_event) => handle_text_input(env, text_event),
        }
    }

    ui_accelerometer::handle_accelerometer(env)
}

/// Typed text goes to the first responder: a text field, or (private fork)
/// another UIKeyInput responder such as an iCade reader.
pub fn handle_text_input(env: &mut Environment, text_event: crate::window::TextInputEvent) {
    use crate::frameworks::foundation::ns_string;
    use crate::objc::release;
    use crate::window::TextInputEvent;

    let responder = env.framework_state.uikit.ui_responder.first_responder;
    let class = msg![env; responder class];
    let ui_text_field_class = env.objc.get_known_class("UITextField", &mut env.mem);
    if !responder.is_null() && env.objc.class_is_subclass_of(class, ui_text_field_class) {
        match text_event {
            TextInputEvent::Text(text) => {
                ui_view::ui_control::ui_text_field::handle_text(env, responder, text)
            }
            TextInputEvent::Backspace => {
                ui_view::ui_control::ui_text_field::handle_backspace(env, responder)
            }
            TextInputEvent::Return => {
                ui_view::ui_control::ui_text_field::handle_return(env, responder)
            }
        }
    } else if !responder.is_null() {
        // Private fork: other UIKeyInput responders, e.g. iCade readers,
        // which expect one character per insertText:.
        let insert_text = env.objc.lookup_selector("insertText:");
        if insert_text.is_some_and(|sel| msg![env; responder respondsToSelector:sel]) {
            let text = match text_event {
                TextInputEvent::Text(text) => text,
                TextInputEvent::Return => "\n".to_string(),
                TextInputEvent::Backspace => String::new(),
            };
            for ch in text.chars() {
                let ch = ns_string::from_rust_string(env, ch.to_string());
                () = msg![env; responder insertText:ch];
                release(env, ch);
            }
        }
    }
}

/// Private fork: the --exit-button and --retry-button. Each press looks at the
/// screen (see [crate::gridrunner::screen_button_action]) and taps a button
/// there, quits, or does nothing; the tap lifts when the button is released.
/// How long a [ScreenAction::Wait] keeps looking.
const SCREEN_BUTTON_WAIT: std::time::Duration = std::time::Duration::from_millis(600);
/// Frames a tap lasts when the button was let go while waiting.
const SCREEN_BUTTON_LATE_TAP_FRAMES: u32 = 6;

struct ScreenButtonWait {
    which: crate::gridrunner::ScreenButton,
    until: std::time::Instant,
    released: bool,
}

thread_local! {
    /// Where each screen button is touching, while it is.
    static SCREEN_BUTTON_TOUCHING: std::cell::RefCell<std::collections::HashMap<u8, (f32, f32)>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    static SCREEN_BUTTON_WAITING: std::cell::RefCell<Option<ScreenButtonWait>> =
        const { std::cell::RefCell::new(None) };
    /// A late tap to lift after some frames: button, frames left.
    static SCREEN_BUTTON_LATE_TAP: std::cell::RefCell<Option<(crate::gridrunner::ScreenButton, u32)>> =
        const { std::cell::RefCell::new(None) };
}

fn screen_button_finger(env: &Environment, which: crate::gridrunner::ScreenButton) -> crate::window::FingerId {
    use crate::gridrunner::ScreenButton;
    let button = match which {
        ScreenButton::Exit => env.options.exit_button,
        ScreenButton::Retry => env.options.retry_button,
    };
    crate::window::FingerId::ButtonToTouch(button.unwrap())
}

/// Lets go of a screen button's touch, if it's touching.
fn screen_button_lift(env: &mut Environment, which: crate::gridrunner::ScreenButton) {
    use crate::window::Event;
    let finger = screen_button_finger(env, which);
    if let Some(at) = SCREEN_BUTTON_TOUCHING.with(|t| t.borrow_mut().remove(&(which as u8))) {
        let touch = std::collections::HashMap::from([(finger, at)]);
        ui_touch::handle_event(env, Event::TouchesUp(touch));
    }
}

/// Acts on what [crate::gridrunner::screen_button_action] says; returns
/// whether it said to wait.
fn screen_button_act(env: &mut Environment, which: crate::gridrunner::ScreenButton) -> bool {
    use crate::gridrunner::{screen_button_action, ScreenAction};
    use crate::window::Event;
    match screen_button_action(env, which) {
        ScreenAction::Quit => {
            echo!("Exit button pressed on the main menu, exiting.");
            ui_application::exit(env);
        }
        ScreenAction::Tap(at) => {
            log!("{:?} button: tapping {:?}.", which, at);
            SCREEN_BUTTON_TOUCHING.with(|t| t.borrow_mut().insert(which as u8, at));
            let touch = std::collections::HashMap::from([(screen_button_finger(env, which), at)]);
            ui_touch::handle_event(env, Event::TouchesDown(touch));
        }
        ScreenAction::Nothing => (),
        ScreenAction::Wait => return true,
    }
    false
}

fn screen_button(env: &mut Environment, which: crate::gridrunner::ScreenButton, down: bool) {
    if !down {
        let waiting = SCREEN_BUTTON_WAITING.with(|w| {
            let mut w = w.borrow_mut();
            w.as_mut().map(|w| w.released = true).is_some()
        });
        if !waiting {
            screen_button_lift(env, which);
        }
        return;
    }
    if screen_button_act(env, which) {
        let until = std::time::Instant::now() + SCREEN_BUTTON_WAIT;
        SCREEN_BUTTON_WAITING.with(|w| *w.borrow_mut() = Some(ScreenButtonWait { which, until, released: false }));
    }
}

/// Private fork: called each frame while an `--exit-button` is set, for a
/// screen button that's waiting ([crate::gridrunner::ScreenAction::Wait]) or
/// a late tap to lift.
pub fn screen_button_frame(env: &mut Environment) {
    let lift = SCREEN_BUTTON_LATE_TAP.with(|l| {
        let mut l = l.borrow_mut();
        match l.as_mut() {
            Some((_, 0)) => l.take().map(|(which, _)| which),
            Some((_, frames)) => {
                *frames -= 1;
                None
            }
            None => None,
        }
    });
    if let Some(which) = lift {
        screen_button_lift(env, which);
    }
    let Some((which, until, released)) =
        SCREEN_BUTTON_WAITING.with(|w| w.borrow().as_ref().map(|w| (w.which, w.until, w.released)))
    else {
        return;
    };
    SCREEN_BUTTON_WAITING.with(|w| *w.borrow_mut() = None);
    if std::time::Instant::now() > until {
        return;
    }
    if screen_button_act(env, which) {
        SCREEN_BUTTON_WAITING.with(|w| *w.borrow_mut() = Some(ScreenButtonWait { which, until, released }));
    } else if released {
        SCREEN_BUTTON_LATE_TAP.with(|l| *l.borrow_mut() = Some((which, SCREEN_BUTTON_LATE_TAP_FRAMES)));
    }
}
