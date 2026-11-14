/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Private fork: `--speed-adjust`, the player's own ship speed for Gridrunner,
//! Super Ox Wars (their `--icade-analog` top speed and acceleration) and
//! Minotron 2112 (its stick move speed). On the game's pause screen LT and RT
//! step it down and up (50% to 150%, in tens), shown on a card at the bottom
//! of the picture; it's kept per game in `touchHLE_speed.txt` next to the
//! exe (a line per app ID: the ID and the factor).

use crate::Environment;
use std::cell::RefCell;
use std::time::{Duration, Instant};

const SPEED_FILE: &str = "touchHLE_speed.txt";
const MIN: i32 = 5;
const MAX: i32 = 15;
/// After the pause check last saw the pause screen, it still counts as
/// paused for this long (Super Ox Wars' EXIT GAME sign pulses).
const PAUSE_HOLD: Duration = Duration::from_millis(1000);
/// The card fades out over this long after the pause screen goes.
const FADE: f32 = 0.3;

#[derive(Default)]
struct State {
    /// Tenths: 10 = the game's (our) own speed.
    tenths: Option<i32>,
    frames: u32,
    paused_seen: Option<Instant>,
    /// Triggers held (LT, RT), for presses.
    held: [bool; 2],
    /// The card's picture, and a number new for each.
    card: Option<(u32, Vec<u8>, (u32, u32))>,
    card_tenths: i32,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn load(env: &Environment) -> i32 {
    let app_id = env.bundle.bundle_identifier();
    let path = crate::paths::user_data_base_path().join(SPEED_FILE);
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .find(|(id, _)| *id == app_id)
        .and_then(|(_, f)| f.trim().parse::<f32>().ok())
        .map_or(10, |f| ((f * 10.0).round() as i32).clamp(MIN, MAX))
}

fn save(env: &Environment, tenths: i32) {
    let app_id = env.bundle.bundle_identifier();
    let path = crate::paths::user_data_base_path().join(SPEED_FILE);
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.split_whitespace().next() != Some(app_id))
        .map(str::to_string)
        .collect();
    lines.push(format!("{} {:.1}", app_id, tenths as f32 / 10.0));
    if let Err(e) = std::fs::write(&path, lines.join("\n") + "\n") {
        log!("Warning: couldn't write {}: {}", path.display(), e);
    }
}

fn tenths(env: &Environment) -> i32 {
    if let Some(t) = STATE.with(|s| s.borrow().tenths) {
        return t;
    }
    let t = load(env);
    STATE.with(|s| s.borrow_mut().tenths = Some(t));
    t
}

/// The speed factor (1.0 = our own tuning).
pub fn factor(env: &Environment) -> f32 {
    if !env.options.speed_adjust {
        return 1.0;
    }
    tenths(env) as f32 / 10.0
}

fn paused_now(env: &mut Environment) -> bool {
    match env.bundle.bundle_identifier() {
        "uk.co.llamasoft.minotron" => crate::minotron::paused(),
        _ => crate::gridrunner::pause_showing(env),
    }
}

/// Each frame: the pause check (every 10 frames) and LT/RT while paused.
pub fn frame(env: &mut Environment) {
    if !env.options.speed_adjust {
        return;
    }
    let n = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.frames = s.frames.wrapping_add(1);
        s.frames
    });
    if n % 10 == 0 && paused_now(env) {
        STATE.with(|s| s.borrow_mut().paused_seen = Some(Instant::now()));
    }
    let window = env.window();
    let pulls = [window.left_trigger(), window.right_trigger()];
    let paused = is_paused();
    let mut step = 0;
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        for (i, &pull) in pulls.iter().enumerate() {
            let held = if s.held[i] { pull > 0.3 } else { pull > 0.6 };
            if held && !s.held[i] && paused {
                step += if i == 0 { -1 } else { 1 };
            }
            s.held[i] = held;
        }
    });
    if step != 0 {
        let old = tenths(env);
        let new = (old + step).clamp(MIN, MAX);
        if new != old {
            STATE.with(|s| s.borrow_mut().tenths = Some(new));
            save(env, new);
            log!("Speed: {}%", new * 10);
        }
    }
}

fn is_paused() -> bool {
    STATE.with(|s| s.borrow().paused_seen.is_some_and(|t| t.elapsed() < PAUSE_HOLD))
}

/// The card to draw this frame, if the pause screen is up: its number (new
/// for each picture), its picture (RGBA, premultiplied) and size, and how
/// opaque it is.
pub fn card<R>(env: &Environment, draw: impl FnOnce(u32, &[u8], (u32, u32), f32) -> R) -> Option<R> {
    if !env.options.speed_adjust {
        return None;
    }
    let seen = STATE.with(|s| s.borrow().paused_seen)?;
    let since = seen.elapsed().saturating_sub(PAUSE_HOLD).as_secs_f32();
    let alpha = 1.0 - since / FADE;
    if alpha <= 0.0 {
        return None;
    }
    let t = tenths(env);
    let stale = STATE.with(|s| {
        let s = s.borrow();
        s.card.is_none() || s.card_tenths != t
    });
    if stale {
        let (w, h) = (900u32, 120u32);
        let text = format!("LT   SHIP SPEED  {}%   RT", t * 10);
        let pixels = crate::achievements::render_card((w, h), &[(54.0, &text, 30.0, [1.0, 0.8, 0.3])]);
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.card = Some((number, pixels, (w, h)));
            s.card_tenths = t;
        });
    }
    STATE.with(|s| {
        let s = s.borrow();
        let (number, pixels, size) = s.card.as_ref()?;
        Some(draw(*number, pixels, *size, alpha.min(1.0)))
    })
}
