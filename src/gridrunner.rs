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
//! stick's position instead. Super Ox Wars (same engine) has the same block,
//! stepping by 0.15.
//!
//! Also `--exit-button=`: one button that taps Exit on the pause screen and
//! quits from the main menu, telling the two apart by what's on screen.

use crate::mem::{MutPtr, MutVoidPtr, Ptr};
use crate::Environment;

/// Where a game's iCade velocity block is and what it works on.
struct Layout {
    bundle_id: &'static str,
    /// The block in the game's per-tick update (r4 = the game struct).
    block_start: u32,
    block_end: u32,
    block_first_instr: [u16; 2],
    /// Code after the call that restores registers live after the block, or
    /// pads the literal that follows to a 4-byte boundary.
    restore: &'static [u16],
    /// Offsets into the game struct.
    velocity_x: u32, // f64, -1 is left
    velocity_y: u32, // f64, -1 is up
    directions: u32, // iCade state bits: 1 up, 2 right, 4 down, 8 left
}

const LAYOUTS: [Layout; 2] = [
    // Starts with `tst.w r1, #0xf` (r1 = iCade state).
    Layout {
        bundle_id: "uk.co.llamasoft.gridrunner",
        block_start: 0x40416,
        block_end: 0x4056c,
        block_first_instr: [0xf011, 0x0f0f],
        restore: &[0xf8d4, 0x00f8], // ldr.w r0, [r4, #0xf8]
        velocity_x: 0x35c,
        velocity_y: 0x364,
        directions: 0x36c,
    },
    // Super Ox Wars: starts with `tst.w r0, #0xa` (r0 = iCade state); only r4
    // is live after it.
    Layout {
        bundle_id: "uk.co.llamasoft.sf",
        block_start: 0x41a68,
        block_end: 0x41bec,
        block_first_instr: [0xf010, 0x0f0a],
        restore: &[0xbf00], // nop (pads the literal to 4 bytes)
        velocity_x: 0x29c,
        velocity_y: 0x2a4,
        directions: 0x2ac,
    },
];

fn layout(env: &Environment) -> Option<&'static Layout> {
    let id = env.bundle.bundle_identifier();
    LAYOUTS.iter().find(|l| l.bundle_id == id)
}

pub fn apply_patch(env: &mut Environment) {
    if env.options.icade_analog.is_none() {
        return;
    }
    let Some(layout) = layout(env) else {
        return;
    };
    let start: MutPtr<u16> = Ptr::from_bits(layout.block_start);
    if env.mem.read(start) != layout.block_first_instr[0]
        || env.mem.read(start + 1) != layout.block_first_instr[1]
    {
        log!("--icade-analog: unexpected code, is this the right version? Not patching.");
        return;
    }

    let host = env.dyld.create_guest_function(
        &mut env.mem,
        "_touchHLE_gridrunner_icade_analog",
        &(icade_analog as fn(&mut Environment, MutVoidPtr)),
    );

    // Thumb-2, replacing the start of the block:
    //   +0   mov r0, r4
    //   +2   ldr r1, [pc, #8]       ; the literal
    //   +4   blx r1                 ; host stub is A32
    //   +6   (restore)
    //        b.w block_end
    //        .word host             ; must be where the ldr looks
    let restore_end = layout.block_start + 6 + 2 * layout.restore.len() as u32;
    let branch_at = restore_end;
    let literal_at = restore_end + 4;
    assert!(literal_at == ((layout.block_start + 2 + 4) & !3) + 8);
    let offset = layout.block_end - (branch_at + 4);
    assert!(offset < 0x1000 && offset % 2 == 0);
    let mut code = vec![0x4620, 0x4902, 0x4788];
    code.extend_from_slice(layout.restore);
    code.extend_from_slice(&[0xf000, 0xb800 | (offset >> 1) as u16]);
    for (i, &halfword) in code.iter().enumerate() {
        env.mem.write(start + i as u32, halfword);
    }
    let literal: MutPtr<u32> = Ptr::from_bits(literal_at);
    env.mem.write(literal, host.addr_with_thumb_bit());
    env.cpu
        .invalidate_cache_range(layout.block_start, layout.block_end - layout.block_start);
    log!("--icade-analog: patched {}'s iCade movement.", layout.bundle_id);
}

/// Called by the patched game once per tick with its game struct.
fn icade_analog(env: &mut Environment, game: MutVoidPtr) {
    let (accel, top_speed) = env.options.icade_analog.unwrap();
    let (accel, top_speed) = (accel as f64, top_speed as f64);
    let (stick_x, stick_y) = env.window().left_stick(&env.options);

    let layout = layout(env).unwrap();
    let game = game.to_bits();
    let vx_ptr: MutPtr<f64> = Ptr::from_bits(game + layout.velocity_x);
    let vy_ptr: MutPtr<f64> = Ptr::from_bits(game + layout.velocity_y);
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
    let dirs_ptr: MutPtr<u32> = Ptr::from_bits(game + layout.directions);
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

/// Super Ox Wars: the pause screen's grey EXIT GAME sign and where to tap it,
/// the menu's grey START button, and the attract logo's blue and red halves.
const SOW_EXIT_SIGN: (u32, u32, u32, u32) = (285, 590, 195, 100);
const SOW_EXIT_TAP: (f32, f32) = (382.0, 641.0);
const SOW_START_BUTTON: (u32, u32, u32, u32) = (200, 700, 370, 95);
const SOW_LOGO_BLUE: (u32, u32, u32, u32) = (275, 240, 80, 60);
const SOW_LOGO_RED: (u32, u32, u32, u32) = (400, 240, 75, 60);

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
    /// Nothing yet, but look again each frame for a moment: the button may
    /// be fading in (Super Ox Wars' EXIT GAME sign pulses).
    Wait,
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
    if env.bundle.bundle_identifier() == "uk.co.llamasoft.sf" {
        return super_ox_wars_exit_button_action(env);
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
        crate::scores::post_running_score(env);
        ScreenAction::Tap(FIVE_A_DAY_EXIT_TAP)
    } else if hiscore.is_some_and(|f| f > 0.06) && touch.is_some_and(|f| f > 0.08) {
        ScreenAction::Quit
    } else {
        ScreenAction::Nothing
    }
}

/// Super Ox Wars: the EXIT GAME sign means pause, so tap it. It's a
/// see-through grey that pulses, so it's found as pale (any tint, as over the
/// purple floor), measured 0.12-0.79 on the demo's and user's pause screens
/// and at most 0.05 (the ship) elsewhere; at its faintest (0.02) the press
/// waits for it. The START button (grey,
/// 0.45 on the menu, at most 0.01 elsewhere) or the attract logo (blue 0.58
/// and red 0.77, at most 0.05 elsewhere) means quit. Anything else, e.g.
/// playing or the other attract pages, does nothing.
fn super_ox_wars_exit_button_action(env: &mut Environment) -> ScreenAction {
    use crate::frameworks::opengles::eagl::read_screen_pixels;

    let grey = |r: u8, g: u8, b: u8| r.abs_diff(g) < 14 && g.abs_diff(b) < 14 && (70..=170).contains(&r);
    let blue = |r: u8, g: u8, b: u8| b > 150 && r < 90 && g < 120;
    let red = |r: u8, g: u8, b: u8| r > 170 && g < 80 && b < 80;
    let pale = |r: u8, g: u8, b: u8| {
        let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
        (70..=235).contains(&hi) && lo as f32 >= 0.6 * hi as f32
    };
    let exit = read_screen_pixels(env, SOW_EXIT_SIGN).map(|p| fraction(&p, pale));
    let start = read_screen_pixels(env, SOW_START_BUTTON).map(|p| fraction(&p, grey));
    let logo_blue = read_screen_pixels(env, SOW_LOGO_BLUE).map(|p| fraction(&p, blue));
    let logo_red = read_screen_pixels(env, SOW_LOGO_RED).map(|p| fraction(&p, red));
    log!(
        "Exit button: EXIT GAME {:?}, START {:?}, logo blue {:?} red {:?}",
        exit, start, logo_blue, logo_red
    );
    if exit.is_some_and(|f| f > 0.08) {
        ScreenAction::Tap(SOW_EXIT_TAP)
    } else if start.is_some_and(|f| f > 0.25)
        || (logo_blue.is_some_and(|f| f > 0.3) && logo_red.is_some_and(|f| f > 0.3))
    {
        ScreenAction::Quit
    } else {
        ScreenAction::Wait
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
