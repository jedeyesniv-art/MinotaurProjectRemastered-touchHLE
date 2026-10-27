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
use crate::mem::{ConstPtr, MutPtr};
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

/// Where a Neon engine game keeps its running score, and the request its
/// `EAGLView drawView:` turns into `postScore:withCategory:` (command 2). All
/// offsets are from the game object, `*(Neon_Internals::gg_cRect + 0x148c)`.
struct RunningScore {
    app_id: &'static str,
    /// The request's command byte, score and category.
    request: (u32, u32, u32),
    score: ScoreAt,
    category: CategoryAt,
}

enum ScoreAt {
    Object(u32),
    /// In the play object, found by its pointer back to the game object at
    /// `.0` (Five A Day), at `.1` in it.
    PlayObject(u32, u32),
}

enum CategoryAt {
    Zero,
    Object(u32),
    /// Rescue: the mode, plus 2 when the byte at `.1` is set.
    ModeAndFlag(u32, u32),
}

/// Each game's own game-over code, found in its binary. Deflex isn't here:
/// it posts a finished level's score, which a quit mid-level doesn't have.
const RUNNING_SCORES: &[RunningScore] = &[
    RunningScore {
        app_id: "uk.co.llamasoft.Goat",
        request: (0x54, 0x5c, 0x60),
        score: ScoreAt::Object(0x530),
        category: CategoryAt::Zero,
    },
    RunningScore {
        app_id: "uk.co.llamasoft.minos",
        request: (0x54, 0x5c, 0x60),
        score: ScoreAt::Object(0x4a4),
        category: CategoryAt::Object(0x9e4),
    },
    RunningScore {
        app_id: "uk.co.llamasoft.minorescue",
        request: (0x2f, 0x30, 0x34),
        score: ScoreAt::Object(0x364),
        category: CategoryAt::ModeAndFlag(0x348, 0x178),
    },
    RunningScore {
        app_id: "uk.co.llamasoft.tp",
        request: (0x5c, 0x64, 0x68),
        score: ScoreAt::PlayObject(0x4c, 0x50),
        category: CategoryAt::Zero,
    },
];

fn read_word(env: &Environment, addr: u32) -> Option<u32> {
    if addr < 0x1000 {
        return None;
    }
    let bytes = env.mem.get_bytes_fallible(ConstPtr::from_bits(addr), 4)?;
    Some(u32::from_le_bytes(bytes.try_into().unwrap()))
}

/// Called when the pad quits a game from its pause screen: asks the game to
/// post its running score, as its game over would, so it's recorded by
/// [faked_call] on the next frame.
pub fn post_running_score(env: &mut Environment) {
    if !env.options.record_scores {
        return;
    }
    let app_id = env.bundle.bundle_identifier();
    let Some(game) = RUNNING_SCORES.iter().find(|g| g.app_id == app_id) else {
        return;
    };
    let object = env.bins[0]
        .exported_symbols
        .get("__ZN14Neon_Internals8gg_cRectE")
        .and_then(|&var| read_word(env, var))
        .and_then(|rect| read_word(env, rect + 0x148c));
    let Some(object) = object else {
        echo!("Running score: no game object found");
        return;
    };
    let score = match game.score {
        ScoreAt::Object(at) => read_word(env, object + at),
        ScoreAt::PlayObject(back, at) => {
            // The game object's pointers to objects that point back to it.
            let play: Vec<u32> = (0..0x2000)
                .step_by(4)
                .filter_map(|off| read_word(env, object + off))
                .filter(|&p| p != object && read_word(env, p + back) == Some(object))
                .collect();
            log!("Running score: play objects {:x?}", play);
            match play[..] {
                [play] => read_word(env, play + at),
                _ => None,
            }
        }
    };
    let category = match game.category {
        CategoryAt::Zero => Some(0),
        CategoryAt::Object(at) => read_word(env, object + at),
        CategoryAt::ModeAndFlag(mode, flag) => read_word(env, object + mode).map(|mode| {
            let flag = env.mem.read(ConstPtr::<u8>::from_bits(object + flag));
            mode + if flag != 0 { 2 } else { 0 }
        }),
    };
    let (Some(score), Some(category)) = (score, category) else {
        echo!("Running score: couldn't read it");
        return;
    };
    echo!("Running score on quit: {} (category {})", score as i32, category);
    if score as i32 <= 0 {
        return;
    }
    let (command_at, score_at, category_at) = game.request;
    env.mem.write(MutPtr::from_bits(object + score_at), score);
    env.mem.write(MutPtr::from_bits(object + category_at), category);
    env.mem.write(MutPtr::<u8>::from_bits(object + command_at), 2);
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
