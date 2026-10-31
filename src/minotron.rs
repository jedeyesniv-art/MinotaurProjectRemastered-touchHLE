/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Private fork: `--minotron-pad`, twin-stick controller support for
//! Minotron: 2112 (iOS 1.01, armv7, ARM code), through the game's own iCade
//! support, and a touch pointer for everything but play.
//!
//! The game keeps the iCade bits in its save data (`[engine + 0x34] +
//! 0xc94`; the control setting, ADAPTIVE / RIGHT HANDED / LEFT HANDED, is
//! at +0xc98). Its player update (r8 = the player, `[player + 0x164]` = the
//! engine) reads them once a tick:
//! - with no aiming touch, iCade buttons aim the shots: 0x10 left, 0x100
//!   right, 0x40 up, 0x80 down, two together a diagonal. Touch aim is
//!   squared off to the same 8 directions (an axis counts below a ratio of
//!   0.5), so the right stick, snapped to 8, gives up nothing. We set these
//!   bits straight in the game's word.
//! - after the touch movement, a block at 0x5f4d4 sets the velocity (f64s
//!   at +0x9c x, +0xa4 y; -y is up) to +-0.006 per held iCade direction.
//!   Touch movement is the finger's pull, capped at a speed of 0.006. We
//!   replace that block with a call to [tick], which sets the velocity from
//!   the left stick (capped like touch), or as the game did for iCade
//!   directions when the stick is idle.
//!
//! The menus have a live demo behind them, so the player update runs there
//! too: play is told apart by what's on screen (landscape 1024x768 points,
//! measured on the user's screenshots and our attract captures):
//! - pause: yellow UNPAUSE and magenta QUIT GAME (0.42 / 0.44; 0 elsewhere)
//! - play: the yellow LEVEL and magenta LIVES in the score bar (0.32 and
//!   0.34; 0 elsewhere, but a yellow LEVEL alone once passed on an info
//!   text page). Play holds for a few checks without them (explosions,
//!   level changes).
//! - main menu: magenta INFO, cyan PLAY and the blue TOUCH TO SELECT GAME
//!   MODE along the top (0.27+ / 0.22+ / 0.27+; the level select has the
//!   first two too, but no blue line)
//! - level select: magenta BACK where INFO is, blue SELECT LEVEL (0.26)
//! - info: blue BACK where INFO is (0.37; the attract demo up to 0.24) and
//!   the yellow ABOUT MINOTRON: 2112 (0.26; the demo up to 0.06)
//! - anything else: the title, the attract demo, game over and the text
//!   pages from INFO, which have no button: an iCade button press leaves
//!   them (as A did in the PC beta), so Back gives one there.
//! Everywhere but play the left stick moves a pointer and A touches.

use crate::frameworks::opengles::eagl::read_screen_pixels;
use crate::frameworks::uikit::{handle_text_input, ui_application, ui_touch};
use crate::gridrunner::{fraction, from_view_point, from_view_rect};
use crate::mem::{MutPtr, MutVoidPtr, Ptr};
use crate::window::{Event, FingerId, PadInput, TextInputEvent};
use crate::Environment;
use sdl2::controller::Button;
use std::cell::RefCell;
use std::collections::HashMap;

const BUNDLE_ID: &str = "uk.co.llamasoft.minotron";
/// The iCade movement block in the player update: `tst r6, #0xf` up to
/// where it rejoins (0x5f544).
const BLOCK_START: u32 = 0x5f4d4;
const BLOCK_FIRST_INSTR: u32 = 0xe316000f;
const BLOCK_END: u32 = 0x5f544;

const PLAYER_ENGINE: u32 = 0x164;
const ENGINE_SAVE: u32 = 0x34;
const SAVE_ICADE: u32 = 0xc94;
const PLAYER_VX: u32 = 0x9c;
const PLAYER_VY: u32 = 0xa4;
/// The game's top speed, for touch and each iCade direction.
const SPEED: f64 = 0.006;

const FIRE_LEFT: u32 = 0x10;
const FIRE_UP: u32 = 0x40;
const FIRE_DOWN: u32 = 0x80;
const FIRE_RIGHT: u32 = 0x100;
const FIRE_BITS: u32 = FIRE_LEFT | FIRE_UP | FIRE_DOWN | FIRE_RIGHT;

/// The left stick's deadzone (radial); past it, speed rises in a straight
/// line to full at the edge.
const MOVE_DEAD: f32 = 0.15;
/// The right stick aims once pushed this far, and stops below `AIM_OFF`.
const AIM_ON: f32 = 0.5;
const AIM_OFF: f32 = 0.35;

type Rect = (u32, u32, u32, u32);
const UNPAUSE_BOX: Rect = (370, 325, 340, 50);
const QUIT_BOX: Rect = (320, 582, 440, 50);
const LEVEL_LABEL: Rect = (440, 15, 110, 40);
const LIVES_LABEL: Rect = (740, 15, 105, 40);
const INFO_BOX: Rect = (225, 665, 80, 40);
const PLAY_BOX: Rect = (710, 665, 85, 40);
const TOP_LINE: Rect = (140, 5, 780, 25);
const SECOND_LINE: Rect = (395, 68, 290, 30);
const ABOUT_LINE: Rect = (275, 255, 490, 40);
/// Checks (every 10 frames) without the score bar before play is over.
const PLAY_MISSES: u32 = 3;

// Where to tap (landscape points).
const SCORE_BAR: (f32, f32) = (512.0, 20.0);
const UNPAUSE: (f32, f32) = (541.0, 350.0);
const QUIT_GAME: (f32, f32) = (541.0, 607.0);
/// BACK on the info page and the level select (INFO on the main menu).
const BACK: (f32, f32) = (263.0, 684.0);

pub fn apply_patch(env: &mut Environment) {
    if !env.options.minotron_pad || env.bundle.bundle_identifier() != BUNDLE_ID {
        return;
    }
    let start: MutPtr<u32> = Ptr::from_bits(BLOCK_START);
    if env.mem.read(start) != BLOCK_FIRST_INSTR {
        log!("--minotron-pad: unexpected code, is this Minotron 2112 1.01? Not patching.");
        return;
    }
    let host = env.dyld.create_guest_function(
        &mut env.mem,
        "_touchHLE_minotron_tick",
        &(tick as fn(&mut Environment, MutVoidPtr)),
    );
    // ARM:
    //   +0   mov r0, r8
    //   +4   ldr r1, [pc, #4]    ; the literal at +16
    //   +8   blx r1
    //   +12  b BLOCK_END
    //   +16  .word host
    // Only r8, fp, sl and sp are live after the block (and the callee-saved
    // VFP registers).
    let branch = 0xea000000 | ((BLOCK_END - (BLOCK_START + 12 + 8)) / 4);
    let code = [0xe1a00008, 0xe59f1004, 0xe12fff31, branch, host.addr_with_thumb_bit()];
    for (i, &word) in code.iter().enumerate() {
        env.mem.write(start + i as u32, word);
    }
    env.cpu.invalidate_cache_range(BLOCK_START, BLOCK_END - BLOCK_START);
    log!("--minotron-pad: patched Minotron's iCade movement for the sticks.");
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Screen {
    Play,
    Pause,
    MainMenu,
    LevelSelect,
    Info,
    /// The title, the attract demo, game over, the INFO text pages and
    /// anything else.
    Other,
}

#[derive(Default)]
struct State {
    frames: u32,
    /// The screen at the last check (every 10 frames).
    screen: Option<Screen>,
    /// Play (or pause) was seen, and how many checks since without it.
    playing: bool,
    misses: u32,
    /// The frame of the last player update, and where the game's iCade word
    /// was then.
    last_tick: Option<u32>,
    icade: Option<u32>,
    /// The fire bits we've set in the game's word.
    fire: u32,
    /// The left stick moved the player last tick (so letting go stops it).
    moving: bool,
    /// iCade letters held by pad inputs (to release).
    held_letters: HashMap<PadInput, &'static str>,
    /// Touches held by pad inputs.
    held_touches: HashMap<PadInput, (FingerId, (f32, f32))>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn score(env: &mut Environment, rect: Rect, test: impl Fn(u8, u8, u8) -> bool) -> f32 {
    let rect = from_view_rect(env, rect);
    read_screen_pixels(env, rect).map_or(0.0, |p| fraction(&p, test))
}

/// The screen judged by its colours alone.
fn recognise_colours(env: &mut Environment) -> Screen {
    let magenta = |r: u8, g: u8, b: u8| r > 150 && g < 90 && b > 150;
    let cyan = |r: u8, g: u8, b: u8| r < 120 && g > 150 && b > 150;
    let yellow = |r: u8, g: u8, b: u8| r > 150 && g > 150 && b < 100;
    let blue = |r: u8, g: u8, b: u8| r < 110 && g < 140 && b > 170 && b > r + 60;
    let scores = [
        score(env, UNPAUSE_BOX, yellow),
        score(env, QUIT_BOX, magenta),
        score(env, LEVEL_LABEL, yellow),
        score(env, LIVES_LABEL, magenta),
        score(env, INFO_BOX, magenta),
        score(env, INFO_BOX, blue),
        score(env, PLAY_BOX, cyan),
        score(env, TOP_LINE, blue),
        score(env, SECOND_LINE, blue),
        score(env, ABOUT_LINE, yellow),
    ];
    log_dbg!("Minotron pad scores: {:?}", scores);
    let [unpause, quit, level, lives, info_magenta, info_blue, play, top, second, about] = scores;
    if unpause > 0.2 && quit > 0.2 {
        Screen::Pause
    } else if level > 0.15 && lives > 0.15 {
        Screen::Play
    } else if info_magenta > 0.15 && play > 0.1 && top > 0.12 {
        Screen::MainMenu
    } else if info_magenta > 0.15 && second > 0.12 {
        Screen::LevelSelect
    } else if info_blue > 0.2 && about > 0.15 {
        Screen::Info
    } else {
        Screen::Other
    }
}

fn screen_now(env: &mut Environment) -> Screen {
    let colours = recognise_colours(env);
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        match colours {
            Screen::Play | Screen::Pause => {
                s.playing = true;
                s.misses = 0;
                colours
            }
            Screen::Other if s.playing && s.misses + 1 < PLAY_MISSES => {
                s.misses += 1;
                Screen::Play
            }
            _ => {
                s.playing = false;
                colours
            }
        }
    })
}

fn current_screen() -> Option<Screen> {
    STATE.with(|s| s.borrow().screen)
}

/// Called by the patched game once per player update (in play, and behind
/// the menus).
fn tick(env: &mut Environment, player: MutVoidPtr) {
    let player = player.to_bits();
    let engine: u32 = env.mem.read(Ptr::<u32, false>::from_bits(player + PLAYER_ENGINE));
    let save: u32 = env.mem.read(Ptr::<u32, false>::from_bits(engine + ENGINE_SAVE));
    let icade_ptr: MutPtr<u32> = Ptr::from_bits(save + SAVE_ICADE);
    let icade = env.mem.read(icade_ptr);
    let vx_ptr: MutPtr<f64> = Ptr::from_bits(player + PLAYER_VX);
    let vy_ptr: MutPtr<f64> = Ptr::from_bits(player + PLAYER_VY);
    // Outside play the left stick is the pointer's, and the right does
    // nothing.
    let play = current_screen() == Some(Screen::Play);

    // Movement.
    let (sx, sy) = if play { env.window().left_stick_raw() } else { (0.0, 0.0) };
    let mag = (sx * sx + sy * sy).sqrt();
    let moving = STATE.with(|s| s.borrow().moving);
    if mag > MOVE_DEAD {
        let speed = ((mag - MOVE_DEAD) / (1.0 - MOVE_DEAD)).min(1.0) as f64 * SPEED;
        env.mem.write(vx_ptr, sx as f64 / mag as f64 * speed);
        env.mem.write(vy_ptr, sy as f64 / mag as f64 * speed);
        STATE.with(|s| s.borrow_mut().moving = true);
    } else {
        if moving {
            env.mem.write(vx_ptr, 0.0);
            env.mem.write(vy_ptr, 0.0);
            STATE.with(|s| s.borrow_mut().moving = false);
        }
        // The game's own iCade movement (the d-pad's letters, a real iCade).
        if icade & 0xf != 0 {
            let axis = |neg: u32, pos: u32| {
                if icade & pos != 0 {
                    SPEED
                } else if icade & neg != 0 {
                    -SPEED
                } else {
                    0.0
                }
            };
            env.mem.write(vx_ptr, axis(8, 2));
            env.mem.write(vy_ptr, axis(1, 4));
        }
    }

    // Aim: the right stick snapped to 8 directions, set as the iCade fire
    // buttons for the next tick.
    let (ax, ay) = if play { env.window().right_stick_raw() } else { (0.0, 0.0) };
    let amag = (ax * ax + ay * ay).sqrt();
    let was = STATE.with(|s| s.borrow().fire);
    let fire = if amag > AIM_ON || (was != 0 && amag > AIM_OFF) {
        // Sectors of 45 degrees, 0 = right, counting clockwise (y is down).
        let sector = ((ay.atan2(ax).to_degrees() + 360.0 + 22.5) / 45.0) as u32 % 8;
        [
            FIRE_RIGHT,
            FIRE_RIGHT | FIRE_DOWN,
            FIRE_DOWN,
            FIRE_DOWN | FIRE_LEFT,
            FIRE_LEFT,
            FIRE_LEFT | FIRE_UP,
            FIRE_UP,
            FIRE_UP | FIRE_RIGHT,
        ][sector as usize]
    } else {
        0
    };
    // Rewrite on a change, or when something else (the iCade reader)
    // rewrote the word.
    if fire != was || (fire != 0 && icade & FIRE_BITS != fire) {
        env.mem.write(icade_ptr, (icade & !FIRE_BITS) | fire);
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.fire = fire;
        s.last_tick = Some(s.frames);
        s.icade = Some(save + SAVE_ICADE);
    });
}

/// Called each frame: the screen check every 10 frames (the pointer, the
/// bars), and letting go of the fire bits once the player update stops
/// (pause), so the menus don't see buttons held.
pub fn frame(env: &mut Environment) {
    let n = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.frames += 1;
        s.frames
    });
    if n % 10 == 0 {
        let screen = screen_now(env);
        let changed = STATE.with(|s| {
            let mut s = s.borrow_mut();
            let changed = s.screen != Some(screen);
            s.screen = Some(screen);
            changed
        });
        if changed {
            log!("Minotron pad: screen {:?}", screen);
        }
        let play = screen == Screen::Play;
        env.window_mut().set_pad_cursor_enabled(!play);
        // Play's black field and colourful sprites make a messy glow: black
        // bars there, the glow everywhere else.
        env.window_mut().set_ambient_bars_off(play || screen == Screen::Pause);
    }

    let (fire, icade, ticking) = STATE.with(|s| {
        let s = s.borrow();
        let ticking = s.last_tick.is_some_and(|t| s.frames.saturating_sub(t) <= 2);
        (s.fire, s.icade, ticking)
    });
    if fire != 0 && !ticking {
        if let Some(at) = icade {
            let ptr: MutPtr<u32> = Ptr::from_bits(at);
            let word = env.mem.read(ptr);
            env.mem.write(ptr, word & !fire);
        }
        STATE.with(|s| s.borrow_mut().fire = 0);
        log_dbg!("Minotron pad: fire let go (player update stopped)");
    }
}

/// iCade joystick letters (pressed, released): the d-pad, which also changes
/// the difficulty and level on the menus.
const ICADE_UP: (&str, &str) = ("w", "e");
const ICADE_RIGHT: (&str, &str) = ("d", "c");
const ICADE_DOWN: (&str, &str) = ("x", "z");
const ICADE_LEFT: (&str, &str) = ("a", "q");
/// iCade button A: leaves the INFO text pages.
const ICADE_A: (&str, &str) = ("y", "t");

enum Action {
    Tap((f32, f32)),
    /// Hold an iCade button while the pad's is held.
    Icade((&'static str, &'static str)),
    Quit,
    Nothing,
}

fn action(screen: Screen, button: Button) -> Action {
    use Button as B;
    match (screen, button) {
        (Screen::Play, B::Start) => Action::Tap(SCORE_BAR),
        (Screen::Pause, B::Start) => Action::Tap(UNPAUSE),
        (Screen::Pause, B::Back) => Action::Tap(QUIT_GAME),
        (Screen::MainMenu, B::Back) => Action::Quit,
        (Screen::Info | Screen::LevelSelect, B::Back) => Action::Tap(BACK),
        (Screen::Other, B::Back) => Action::Icade(ICADE_A),
        _ => Action::Nothing,
    }
}

pub fn handle_input(env: &mut Environment, input: PadInput, down: bool) {
    use Button as B;
    if !down {
        if let Some(letter) = STATE.with(|s| s.borrow_mut().held_letters.remove(&input)) {
            handle_text_input(env, TextInputEvent::Text(letter.to_string()));
        }
        if let Some((finger, at)) = STATE.with(|s| s.borrow_mut().held_touches.remove(&input)) {
            ui_touch::handle_event(env, Event::TouchesUp(HashMap::from([(finger, at)])));
        }
        return;
    }
    // A goes to the pointer outside play (the window takes it), and does
    // nothing in play; the left stick's left/right events only come in play,
    // where the stick moves.
    let PadInput::Button(button) = input else { return };
    let letters = match button {
        B::DPadUp => Some(ICADE_UP),
        B::DPadDown => Some(ICADE_DOWN),
        B::DPadLeft => Some(ICADE_LEFT),
        B::DPadRight => Some(ICADE_RIGHT),
        _ => None,
    };
    if let Some((press, release)) = letters {
        handle_text_input(env, TextInputEvent::Text(press.to_string()));
        STATE.with(|s| s.borrow_mut().held_letters.insert(input, release));
        return;
    }
    // Check now: a press can come just after the screen changed.
    let screen = screen_now(env);
    STATE.with(|s| s.borrow_mut().screen = Some(screen));
    let action = action(screen, button);
    log!("Minotron pad: {:?} on {:?}", button, screen);
    match action {
        Action::Tap(point) => {
            let at = from_view_point(env, point);
            let finger = FingerId::Pad(button as u8);
            STATE.with(|s| s.borrow_mut().held_touches.insert(input, (finger, at)));
            ui_touch::handle_event(env, Event::TouchesDown(HashMap::from([(finger, at)])));
        }
        Action::Icade((press, release)) => {
            handle_text_input(env, TextInputEvent::Text(press.to_string()));
            STATE.with(|s| s.borrow_mut().held_letters.insert(input, release));
        }
        Action::Quit => {
            echo!("Back pressed on Minotron's main menu, exiting.");
            ui_application::exit(env);
        }
        Action::Nothing => (),
    }
}
