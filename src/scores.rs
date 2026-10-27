/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Private fork: `--record-scores`. OpenFeint is faked (`--fake-classes=OF`),
//! so a game's `[OFHighScoreService setHighScore:... forLeaderboard:...]`
//! goes nowhere; instead the score is added to `touchHLE_scores.txt` in the
//! touchHLE folder, which the launcher reads. One line per score, separated
//! by tabs: Unix time, app ID, leaderboard ID, score, player name
//! (`--player-name=`).

use crate::cpu::Cpu;
use crate::frameworks::foundation::ns_string;
use crate::mem::ConstPtr;
use crate::objc::{id, nil};
use crate::Environment;
use std::io::Write;

pub const SCORES_FILE: &str = "touchHLE_scores.txt";

/// Called for every message to a faked class, before it's dropped.
pub fn faked_call(env: &mut Environment, class_name: &str, selector: &str) {
    if !env.options.record_scores
        || class_name != "OFHighScoreService"
        || !selector.starts_with("setHighScore:")
    {
        return;
    }
    // + setHighScore:(int64_t)score ... forLeaderboard:(NSString *)id ...:
    // the score is in r2:r3, the arguments after it on the stack, one word
    // each (all objects or references).
    let regs = env.cpu.regs();
    let score = (regs[2] as u64 | ((regs[3] as u64) << 32)) as i64;
    let sp = regs[Cpu::SP];
    let board = selector
        .split(':')
        .position(|label| label == "forLeaderboard")
        .map(|i| env.mem.read(ConstPtr::<id>::from_bits(sp + 4 * (i as u32 - 1))))
        .filter(|&string| string != nil)
        .map(|string| ns_string::to_rust_string(env, string).to_string())
        .unwrap_or_default();
    record(env, &board, score);
}

fn clean(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

fn record(env: &mut Environment, board: &str, score: i64) {
    let app_id = env.bundle.bundle_identifier().to_string();
    let name = clean(&env.options.player_name);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let line = format!("{time}\t{app_id}\t{}\t{score}\t{name}\n", clean(board));
    echo!("Score recorded: {} on {:?} for {}", score, board, name);
    let path = crate::paths::user_data_base_path().join(SCORES_FILE);
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(line.as_bytes()));
    if let Err(e) = result {
        echo!("Warning: couldn't record the score in {}: {}", path.display(), e);
    }
}
