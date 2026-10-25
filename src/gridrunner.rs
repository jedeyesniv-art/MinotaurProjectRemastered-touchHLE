/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Private Gridrunner fork: analog iCade movement (`--icade-analog=`).
//!
//! Gridrunner 1.02 (armv7) moves the ship with two velocities in [-1, 1] that
//! its iCade code steps by 0.08 per tick while a direction is held. We replace
//! that block with a call to [icade_analog], which eases them towards the left
//! stick's position instead.
//!
//! Also `--exit-button=`: one button that taps Exit on the pause screen and
//! quits from the main menu, telling the two apart by what's on screen.

use crate::mem::{MutPtr, MutVoidPtr, Ptr};
use crate::Environment;

/// The iCade velocity block in the game's per-tick update, which starts with
/// `tst.w r1, #0xf` (r1 = iCade state, r4 = the game struct).
const BLOCK_START: u32 = 0x40416;
const BLOCK_END: u32 = 0x4056c;
const BLOCK_FIRST_INSTR: [u16; 2] = [0xf011, 0x0f0f];

/// Offsets into the game struct.
const VELOCITY_X: u32 = 0x35c; // f64, -1 is left
const VELOCITY_Y: u32 = 0x364; // f64, -1 is up
const DIRECTIONS: u32 = 0x36c; // iCade state bits: 1 up, 2 right, 4 down, 8 left

pub fn apply_patch(env: &mut Environment) {
    if env.options.icade_analog.is_none()
        || env.bundle.bundle_identifier() != "uk.co.llamasoft.gridrunner"
    {
        return;
    }
    let start: MutPtr<u16> = Ptr::from_bits(BLOCK_START);
    if env.mem.read(start) != BLOCK_FIRST_INSTR[0] || env.mem.read(start + 1) != BLOCK_FIRST_INSTR[1]
    {
        log!("--icade-analog: unexpected code, is this Gridrunner 1.02? Not patching.");
        return;
    }

    let host = env.dyld.create_guest_function(
        &mut env.mem,
        "_touchHLE_gridrunner_icade_analog",
        &(icade_analog as fn(&mut Environment, MutVoidPtr)),
    );

    // Thumb-2, replacing the start of the block:
    //   +0   mov r0, r4
    //   +2   ldr r1, [pc, #8]       ; the literal at +14
    //   +4   blx r1                 ; host stub is A32
    //   +6   ldr.w r0, [r4, #0xf8]  ; r0 is live after the block
    //   +10  b.w BLOCK_END
    //   +14  .word host
    const BRANCH_AT: u32 = BLOCK_START + 10;
    const LITERAL_AT: u32 = BLOCK_START + 14;
    const _: () = assert!(LITERAL_AT % 4 == 0);
    let offset = BLOCK_END - (BRANCH_AT + 4);
    assert!(offset < 0x1000 && offset % 2 == 0);
    let code: [u16; 7] = [
        0x4620,
        0x4902,
        0x4788,
        0xf8d4,
        0x00f8,
        0xf000,
        0xb800 | (offset >> 1) as u16,
    ];
    for (i, &halfword) in code.iter().enumerate() {
        env.mem.write(start + i as u32, halfword);
    }
    let literal: MutPtr<u32> = Ptr::from_bits(LITERAL_AT);
    env.mem.write(literal, host.addr_with_thumb_bit());
    env.cpu.invalidate_cache_range(BLOCK_START, BLOCK_END - BLOCK_START);
    log!("--icade-analog: patched Gridrunner's iCade movement.");
}

/// Called by the patched game once per tick with its game struct.
fn icade_analog(env: &mut Environment, game: MutVoidPtr) {
    let (accel, top_speed) = env.options.icade_analog.unwrap();
    let (accel, top_speed) = (accel as f64, top_speed as f64);
    let (stick_x, stick_y) = env.window().left_stick(&env.options);

    let game = game.to_bits();
    let vx_ptr: MutPtr<f64> = Ptr::from_bits(game + VELOCITY_X);
    let vy_ptr: MutPtr<f64> = Ptr::from_bits(game + VELOCITY_Y);
    // Only speeding up is gradual: easing off, letting go and reversing take
    // effect at once (a reversal keeps the speed the ship already had).
    let approach = |v: f64, stick: f32| {
        let target = stick as f64 * top_speed;
        let speed = (v.abs() + accel).min(target.abs());
        speed.copysign(target)
    };
    let vx = approach(env.mem.read(vx_ptr), stick_x);
    let vy = approach(env.mem.read(vy_ptr), stick_y);
    env.mem.write(vx_ptr, vx);
    env.mem.write(vy_ptr, vy);

    // The game only uses these velocities while an iCade direction is held,
    // so keep one "held" for as long as the ship is still moving.
    let mut dirs = 0;
    if vy < 0.0 {
        dirs |= 1;
    }
    if vx > 0.0 {
        dirs |= 2;
    }
    if vy > 0.0 {
        dirs |= 4;
    }
    if vx < 0.0 {
        dirs |= 8;
    }
    let dirs_ptr: MutPtr<u32> = Ptr::from_bits(game + DIRECTIONS);
    let state = env.mem.read(dirs_ptr);
    env.mem.write(dirs_ptr, (state & !0xf) | dirs);
}

/// Where the main menu's START! button and the pause screen's Exit sign are
/// (portrait screen points), and where to tap Exit.
const START_BUTTON: (u32, u32, u32, u32) = (200, 780, 370, 100);
const EXIT_SIGN: (u32, u32, u32, u32) = (290, 582, 188, 100);
const EXIT_TAP: (f32, f32) = (384.0, 633.0);

/// Five A Day (same engine): its pause screen's Exit sign and where to tap
/// it, and the title screen's purple LOCAL HISCORE and lavender TOUCH TO PLAY
/// lines.
const FIVE_A_DAY_EXIT_SIGN: (u32, u32, u32, u32) = (290, 590, 188, 100);
const FIVE_A_DAY_EXIT_TAP: (f32, f32) = (384.0, 639.0);
const FIVE_A_DAY_HISCORE: (u32, u32, u32, u32) = (160, 740, 400, 30);
const FIVE_A_DAY_TOUCH_TO_PLAY: (u32, u32, u32, u32) = (214, 838, 346, 30);

/// Private fork: the controller buttons that act on what's on screen.
#[derive(Copy, Clone, Debug)]
pub enum ScreenButton {
    /// `--exit-button`: Exit from the pause screen, quit from the main menu.
    Exit,
    /// `--retry-button`: Retry from the pause screen.
    Retry,
}

pub enum ScreenAction {
    /// The main menu is showing: quit the app.
    Quit,
    /// Tap here (in the app's own, portrait, screen points).
    Tap((f32, f32)),
    /// Anything else, e.g. playing: do nothing.
    Nothing,
}

/// Fraction of a block of RGBA pixels that pass a test.
pub(crate) fn fraction(pixels: &[u8], test: impl Fn(u8, u8, u8) -> bool) -> f32 {
    let count = pixels.chunks_exact(4).filter(|p| test(p[0], p[1], p[2])).count();
    count as f32 / (pixels.len() / 4).max(1) as f32
}

pub fn screen_button_action(env: &mut Environment, which: ScreenButton) -> ScreenAction {
    use crate::window::DeviceOrientation;

    match (env.bundle.bundle_identifier(), which) {
        ("uk.co.llamasoft.deflex", _) => return deflex_screen_button_action(env, which),
        (_, ScreenButton::Retry) => return ScreenAction::Nothing,
        _ => (),
    }
    if env.window().current_rotation() != DeviceOrientation::Portrait {
        return ScreenAction::Nothing;
    }
    if env.bundle.bundle_identifier() == "uk.co.llamasoft.tp" {
        return five_a_day_exit_button_action(env);
    }
    gridrunner_exit_button_action(env)
}

fn gridrunner_exit_button_action(env: &mut Environment) -> ScreenAction {
    use crate::frameworks::opengles::eagl::read_screen_pixels;

    // The START! button is solid blue behind white letters.
    let blue = |r: u8, g: u8, b: u8| b > 200 && r < 60 && g < 60;
    if read_screen_pixels(env, START_BUTTON).is_some_and(|p| fraction(&p, blue) > 0.4) {
        return ScreenAction::Quit;
    }
    // The Exit sign is mid grey behind white letters.
    let grey = |r: u8, g: u8, b: u8| {
        r.abs_diff(g) < 12 && g.abs_diff(b) < 12 && (60..120).contains(&r)
    };
    if read_screen_pixels(env, EXIT_SIGN).is_some_and(|p| fraction(&p, grey) > 0.35) {
        return ScreenAction::Tap(EXIT_TAP);
    }
    ScreenAction::Nothing
}

/// Five A Day: the Exit sign (blue-grey, measured 0.48 on the pause screen
/// and at most 0.08 elsewhere) means pause, so tap Exit. Both title lines
/// (measured 0.13-0.19 and 0.20-0.22 there, 0 on the other attract pages; the
/// pause screen's TOUCH button matches the lavender only) mean the title
/// screen, so quit. Anything else, e.g. playing, does nothing.
fn five_a_day_exit_button_action(env: &mut Environment) -> ScreenAction {
    use crate::frameworks::opengles::eagl::read_screen_pixels;

    let exit_grey = |r: u8, g: u8, b: u8| {
        r.abs_diff(g) < 14 && (12..=45).contains(&(b as i32 - r as i32)) && (65..=130).contains(&r)
    };
    let purple = |r: u8, g: u8, b: u8| (95..=170).contains(&r) && g < 80 && b > 190;
    let lavender = |r: u8, g: u8, b: u8| {
        (100..=175).contains(&r) && (110..=175).contains(&g) && b > 190 && r.abs_diff(g) < 30
    };
    let exit = read_screen_pixels(env, FIVE_A_DAY_EXIT_SIGN).map(|p| fraction(&p, exit_grey));
    let hiscore = read_screen_pixels(env, FIVE_A_DAY_HISCORE).map(|p| fraction(&p, purple));
    let touch = read_screen_pixels(env, FIVE_A_DAY_TOUCH_TO_PLAY).map(|p| fraction(&p, lavender));
    log!("Exit button: Exit sign {:?}, LOCAL HISCORE {:?}, TOUCH TO PLAY {:?}", exit, hiscore, touch);
    if exit.is_some_and(|f| f > 0.3) {
        ScreenAction::Tap(FIVE_A_DAY_EXIT_TAP)
    } else if hiscore.is_some_and(|f| f > 0.06) && touch.is_some_and(|f| f > 0.08) {
        ScreenAction::Quit
    } else {
        ScreenAction::Nothing
    }
}

/// Converts a point in the screen as the user sees it (e.g. 1024x768 for a
/// landscape iPad app) to the app's own portrait screen points, the way
/// mouse input is converted.
pub(crate) fn from_view_point(env: &mut Environment, (x, y): (f32, f32)) -> (f32, f32) {
    let window = env.window();
    let (w, h) = window.size_unrotated_unscaled();
    let (vw, vh) = match window.current_rotation() {
        crate::window::DeviceOrientation::LandscapeLeft
        | crate::window::DeviceOrientation::LandscapeRight => (h, w),
        _ => (w, h),
    };
    let matrix = window.rotation_matrix().inverse().unwrap();
    let [px, py] = matrix.transform([x / vw as f32 - 0.5, y / vh as f32 - 0.5]);
    (((px + 0.5) * w as f32).round(), ((py + 0.5) * h as f32).round())
}

/// [from_view_point] for a rectangle (x, y, width, height).
pub(crate) fn from_view_rect(env: &mut Environment, (x, y, w, h): (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    let a = from_view_point(env, (x as f32, y as f32));
    let b = from_view_point(env, ((x + w) as f32, (y + h) as f32));
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32)
}

/// Deflex (landscape, 1024x768 as seen): its pause screen has RESUME, RETRY
/// and QUIT buttons down the middle, each grey with white letters (white
/// measured 0.11-0.14 in each box there, at most 0.025 in play). B taps QUIT,
/// X taps RETRY. Anywhere else both do nothing.
const DEFLEX_BUTTONS: [(u32, u32, u32, u32); 3] =
    [(425, 270, 185, 75), (440, 505, 150, 75), (440, 622, 150, 70)];
const DEFLEX_RETRY_TAP: (f32, f32) = (512.0, 543.0);
const DEFLEX_QUIT_TAP: (f32, f32) = (512.0, 657.0);

fn deflex_screen_button_action(env: &mut Environment, which: ScreenButton) -> ScreenAction {
    use crate::frameworks::opengles::eagl::read_screen_pixels;

    let white = |r: u8, g: u8, b: u8| r > 215 && g > 215 && b > 215;
    let mut scores = [None; 3];
    for (score, rect) in scores.iter_mut().zip(DEFLEX_BUTTONS) {
        let rect = from_view_rect(env, rect);
        *score = read_screen_pixels(env, rect).map(|p| fraction(&p, white));
    }
    log!("{:?} button: Deflex pause buttons white {:?}", which, scores);
    if !scores.iter().all(|s| s.is_some_and(|f| f > 0.06)) {
        return ScreenAction::Nothing;
    }
    let tap = match which {
        ScreenButton::Exit => DEFLEX_QUIT_TAP,
        ScreenButton::Retry => DEFLEX_RETRY_TAP,
    };
    ScreenAction::Tap(from_view_point(env, tap))
}
