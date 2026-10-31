/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Private fork: `--record-achievements`. The games' achievement unlocks go
//! to OpenFeint or Game Center, both gone, so each one is added to
//! `touchHLE_achievements.txt` in the touchHLE folder, which the launcher
//! reads. One line per unlock, separated by tabs: Unix time, app ID,
//! achievement ID, player name (`--player-name=`).
//!
//! - OpenFeint games (faked, `--fake-classes=OF`) do
//!   `[[OFAchievement achievement:@"id"] unlock]` (Goat Up, from its
//!   `EAGLView drawView:` commands 4-13), or `unlockAndDefer` or
//!   `updateProgressionComplete:andShowNotification:`: the faked
//!   `achievement:` call leaves the ID here ([faked_call]), and the unlock,
//!   sent to the nil it returned, records it ([nil_message]).
//! - The Game Center games (Gridrunner, Super Ox Wars) post them through
//!   `runSubmitter`, which [crate::scores] lets through; the ID arrives at
//!   [record] from its nil `submitAchievement:percentComplete:`.
//!
//! A new unlock (not one this player already has) also shows a banner at
//! the top of the picture for a few seconds ([banner], drawn by
//! `eagl.rs` with [crate::gles::present::draw_banner]), titled from the
//! launcher's `launcher_text.txt` beside the touchHLE folder.

use crate::frameworks::foundation::ns_string;
use crate::objc::{id, nil};
use crate::Environment;
use std::cell::RefCell;
use std::io::Write;
use std::time::Instant;

pub const ACHIEVEMENTS_FILE: &str = "touchHLE_achievements.txt";

thread_local! {
    /// The ID from the last faked `+[OFAchievement achievement:]`.
    static PENDING: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The banner being shown: a number new for each, its picture (RGBA,
    /// premultiplied), its size, and when it appeared.
    static BANNER: RefCell<Option<(u32, Vec<u8>, (u32, u32), Instant)>> = const { RefCell::new(None) };
}

/// How long the banner shows, and its fades in and out (seconds).
const BANNER_TIME: f32 = 4.5;
const BANNER_FADE_IN: f32 = 0.25;
const BANNER_FADE_OUT: f32 = 0.6;

fn string(env: &mut Environment, string: id) -> Option<String> {
    (string != nil).then(|| ns_string::to_rust_string(env, string).to_string())
}

/// Called for every message to a faked class, before it's dropped.
pub fn faked_call(env: &mut Environment, class_name: &str, selector: &str) {
    if !env.options.record_achievements || class_name != "OFAchievement" || selector != "achievement:" {
        return;
    }
    let achievement = string(env, id::from_bits(env.cpu.regs()[2]));
    PENDING.with(|p| *p.borrow_mut() = achievement);
}

/// Called for every message to nil.
pub fn nil_message(env: &mut Environment, selector: &str) {
    if !env.options.record_achievements {
        return;
    }
    let percent = match selector {
        "unlock" | "unlockAndDefer" => 100.0,
        // - updateProgressionComplete:(double)percent andShowNotification:(BOOL)
        "updateProgressionComplete:andShowNotification:" => {
            let regs = env.cpu.regs();
            f64::from_bits(regs[2] as u64 | ((regs[3] as u64) << 32))
        }
        _ => return,
    };
    let Some(achievement) = PENDING.with(|p| p.borrow_mut().take()) else {
        return;
    };
    log!("Achievement {} ({}) at {}%", achievement, selector, percent);
    if percent >= 100.0 {
        record(env, &achievement);
    }
}

fn clean(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// Adds an unlock to the achievements file, and shows the banner, unless
/// this player has it already (the games can unlock one more than once).
pub fn record(env: &mut Environment, achievement: &str) {
    let app_id = env.bundle.bundle_identifier().to_string();
    let name = clean(&env.options.player_name);
    let path = crate::paths::user_data_base_path().join(ACHIEVEMENTS_FILE);
    let known = std::fs::read_to_string(&path).unwrap_or_default().lines().any(|line| {
        let f: Vec<&str> = line.split('\t').collect();
        f.len() >= 4 && f[1] == app_id && f[2] == clean(achievement) && f[3] == name
    });
    if known {
        echo!("Achievement {} again for {}, already recorded", achievement, name);
        return;
    }
    show_banner(&title(&app_id, achievement));
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let line = format!("{time}\t{app_id}\t{}\t{name}\n", clean(achievement));
    echo!("Achievement unlocked: {} for {}", achievement, name);
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = result {
        log!("Warning: couldn't write {}: {}", path.display(), e);
    }
}

/// The achievement's title from the launcher's words ("achievement ID:
/// Title | goal" in the game's section), if it can be found.
fn title(app_id: &str, achievement: &str) -> Option<String> {
    let base = crate::paths::user_data_base_path();
    let text = std::fs::read_to_string(base.parent()?.join("launcher_text.txt")).ok()?;
    let mut in_game = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with("== ") {
            in_game = false;
        } else if let Some(app) = line.strip_prefix("app:") {
            in_game = app.trim() == app_id;
        } else if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let is_it = key.strip_prefix("secret ").unwrap_or(key).strip_prefix("achievement ").map(str::trim) == Some(achievement);
            if in_game && is_it {
                return Some(value.split('|').next()?.trim().to_string()).filter(|t| !t.is_empty());
            }
        }
    }
    None
}

/// Draws the banner's picture and starts showing it.
fn show_banner(title: &Option<String>) {
    use crate::font::{Font, TextAlignment};
    let (w, h) = (1000u32, 180u32);
    let mut px = vec![0f32; (w * h * 4) as usize];
    // a navy card with a gold edge and rounded corners
    let (radius, edge) = (28.0f32, 4.0f32);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let dx = (radius - fx).max(fx - (w as f32 - radius)).max(0.0);
            let dy = (radius - fy).max(fy - (h as f32 - radius)).max(0.0);
            let d = radius - (dx * dx + dy * dy).sqrt(); // inside distance from the rim
            let cover = d.clamp(0.0, 1.0);
            let gold = (edge - d + 0.5).clamp(0.0, 1.0);
            let (r, g, b) = (0.03 + gold * 0.97, 0.04 + gold * 0.76, 0.10 + gold * 0.20);
            let a = cover * (0.88 + 0.12 * gold);
            let i = ((y * w + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[r * a, g * a, b * a, a]);
        }
    }
    let mut text = |font: &Font, size: f32, line: &str, top: f32, colour: [f32; 3]| {
        font.draw(size, line, (w as f32 / 2.0, top), None, TextAlignment::Center, |glyph| {
            let (ox, oy) = glyph.origin();
            let (gw, gh) = glyph.dimensions();
            for gy in 0..gh {
                for gx in 0..gw {
                    let (x, y) = (ox as i32 + gx, oy as i32 + gy);
                    if !(0..w as i32).contains(&x) || !(0..h as i32).contains(&y) {
                        continue;
                    }
                    let c = glyph.pixel_at((gx, gy));
                    let i = ((y as u32 * w + x as u32) * 4) as usize;
                    for k in 0..3 {
                        px[i + k] = px[i + k] * (1.0 - c) + colour[k] * c;
                    }
                    px[i + 3] = px[i + 3] * (1.0 - c) + c;
                }
            }
        });
    };
    let font = Font::sans_bold();
    match title {
        Some(title) => {
            text(&font, 34.0, "ACHIEVEMENT UNLOCKED", 24.0, [1.0, 0.8, 0.3]);
            text(&font, 60.0, title, 78.0, [1.0, 1.0, 1.0]);
        }
        None => text(&font, 60.0, "Achievement unlocked!", 52.0, [1.0, 0.8, 0.3]),
    }
    let bytes = px.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    BANNER.with(|b| *b.borrow_mut() = Some((number, bytes, (w, h), Instant::now())));
}

/// The banner to draw this frame, if one is showing: its number (new for
/// each banner), its picture and size, and how opaque it is now.
pub fn banner<R>(draw: impl FnOnce(u32, &[u8], (u32, u32), f32) -> R) -> Option<R> {
    // TOUCHHLE_TEST_BANNER=title shows one at the start (for checking it on
    // screenshots without playing for an unlock)
    static TESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if let Ok(title) = std::env::var("TOUCHHLE_TEST_BANNER") {
        if !TESTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            show_banner(&Some(title));
        }
    }
    BANNER.with(|b| {
        let mut b = b.borrow_mut();
        let (number, bytes, size, shown) = b.as_ref()?;
        let t = shown.elapsed().as_secs_f32();
        if t > BANNER_TIME {
            *b = None;
            return None;
        }
        let alpha = (t / BANNER_FADE_IN).min((BANNER_TIME - t) / BANNER_FADE_OUT).clamp(0.0, 1.0);
        Some(draw(*number, bytes, *size, alpha))
    })
}
