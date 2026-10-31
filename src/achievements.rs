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

use crate::frameworks::foundation::ns_string;
use crate::objc::{id, nil};
use crate::Environment;
use std::cell::RefCell;
use std::io::Write;

pub const ACHIEVEMENTS_FILE: &str = "touchHLE_achievements.txt";

thread_local! {
    /// The ID from the last faked `+[OFAchievement achievement:]`.
    static PENDING: RefCell<Option<String>> = const { RefCell::new(None) };
}

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

/// Adds an unlock to the achievements file.
pub fn record(env: &mut Environment, achievement: &str) {
    let app_id = env.bundle.bundle_identifier().to_string();
    let name = clean(&env.options.player_name);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let line = format!("{time}\t{app_id}\t{}\t{name}\n", clean(achievement));
    echo!("Achievement unlocked: {} for {}", achievement, name);
    let path = crate::paths::user_data_base_path().join(ACHIEVEMENTS_FILE);
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = result {
        log!("Warning: couldn't write {}: {}", path.display(), e);
    }
}
