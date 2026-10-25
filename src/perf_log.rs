/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Private fork: `--perf-log=FILE`, a once-a-second CSV of what the app asks
//! of the GPU and how long the GPU takes over it, for tracking down slowdowns.
//! F9 adds a marker to the next row.
//!
//! GPU times come from `GL_TIME_ELAPSED` timer queries in the context that
//! presents with `presentRenderbuffer:`: one spans the app's drawing, from its
//! first draw or clear after a present up to the next present, and one each
//! spans the [STAGES] of touchHLE's presenting. A timer query also counts time
//! the GPU spends waiting for commands, so the first is an upper bound. They
//! assume the OpenGL 2.1 backend touchHLE uses on desktop.

use touchHLE_gl_bindings::gl21compat as gl;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// `GL_TIME_ELAPSED` from `GL_ARB_timer_query`, which isn't in the OpenGL 2.1
/// bindings.
const TIME_ELAPSED: gl::types::GLenum = 0x88BF;
/// Timer query sets in flight; results arrive a few frames late.
const QUERY_SLOTS: usize = 8;
/// The parts of presenting, in order, each ended by [present_stage] (the last
/// by [present_end]).
pub const STAGES: [&str; 6] = ["copy", "clear", "bezel", "prescale", "frame", "margin"];

static ENABLED: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<Option<PerfLog>> = Mutex::new(None);

/// What the app asked for during the current second.
#[derive(Default)]
pub struct Counters {
    pub draws: u64,
    pub vertices: u64,
    pub texture_upload_bytes: u64,
    pub buffer_upload_bytes: u64,
    pub clears: u64,
    pub framebuffer_binds: u64,
    pub program_binds: u64,
    pub read_pixels: u64,
}

#[derive(Default, Clone, Copy)]
struct QuerySlot {
    /// The app's drawing since the previous present.
    frame: u32,
    /// Whether `frame` was used for the frame that `stages` end.
    frame_used: bool,
    /// touchHLE's presenting, one query per stage.
    stages: [u32; STAGES.len()],
    /// How many of `stages` were used.
    stages_used: usize,
    pending: bool,
}

struct PerfLog {
    file: std::io::BufWriter<std::fs::File>,
    start: Instant,
    second_start: Instant,
    last_present: Option<Instant>,
    frames: u64,
    longest_frame: Duration,
    present_cpu: Duration,
    counters: Counters,
    gpu_frame_ns: u64,
    gpu_stage_ns: [u64; STAGES.len()],
    gpu_samples: u64,
    markers: u32,
    slots: Vec<QuerySlot>,
    slot: usize,
    frame_query_running: bool,
    /// The presenting stage being timed, if presenting.
    stage: Option<usize>,
}

pub fn start(path: &std::path::Path) {
    let file = match std::fs::File::create(path) {
        Ok(file) => file,
        Err(e) => {
            echo!("Warning: Couldn't create perf log {}: {}", path.display(), e);
            return;
        }
    };
    let mut file = std::io::BufWriter::new(file);
    let stage_columns: String = STAGES
        .iter()
        .map(|s| format!(",gpu_present_{s}_ms_per_frame"))
        .collect();
    let _ = writeln!(
        file,
        "unix_ms,seconds,fps,longest_frame_ms,draws_per_frame,vertices_per_frame,\
         texture_upload_kb_per_frame,buffer_upload_kb_per_frame,clears_per_frame,\
         framebuffer_binds_per_frame,program_binds_per_frame,read_pixels,\
         gpu_game_span_ms_per_frame,gpu_present_ms_per_frame{stage_columns},\
         present_cpu_ms_per_frame,markers"
    );
    let now = Instant::now();
    *STATE.lock().unwrap() = Some(PerfLog {
        file,
        start: now,
        second_start: now,
        last_present: None,
        frames: 0,
        longest_frame: Duration::ZERO,
        present_cpu: Duration::ZERO,
        counters: Counters::default(),
        gpu_frame_ns: 0,
        gpu_stage_ns: [0; STAGES.len()],
        gpu_samples: 0,
        markers: 0,
        slots: Vec::new(),
        slot: 0,
        frame_query_running: false,
        stage: None,
    });
    ENABLED.store(true, Ordering::Relaxed);
    echo!("Writing a performance log to {}.", path.display());
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Add to this second's counters, e.g. `count(|c| c.draws += 1)`.
pub fn count(f: impl FnOnce(&mut Counters)) {
    if !enabled() {
        return;
    }
    if let Some(log) = STATE.lock().unwrap().as_mut() {
        f(&mut log.counters);
    }
}

pub fn mark() {
    if let Some(log) = STATE.lock().unwrap().as_mut() {
        log.markers += 1;
        echo!("Perf log marker {} added.", log.markers);
    }
}

/// Call before each of the app's draws and clears, with its context current.
pub unsafe fn app_drawing() {
    let mut state = STATE.lock().unwrap();
    let Some(log) = state.as_mut() else {
        return;
    };
    // No slots until the first present.
    let Some(slot) = log.slots.get_mut(log.slot) else {
        return;
    };
    if !log.frame_query_running && log.stage.is_none() && !slot.frame_used {
        gl::BeginQuery(TIME_ELAPSED, slot.frame);
        slot.frame_used = true;
        log.frame_query_running = true;
    }
}

/// Call at the start of presenting, with the presenting context current.
pub unsafe fn present_begin() {
    let mut state = STATE.lock().unwrap();
    let Some(log) = state.as_mut() else {
        return;
    };
    if log.slots.is_empty() {
        let per_slot = 1 + STAGES.len();
        let mut ids = vec![0u32; QUERY_SLOTS * per_slot];
        gl::GenQueries(ids.len() as _, ids.as_mut_ptr());
        log.slots = ids
            .chunks(per_slot)
            .map(|ids| QuerySlot {
                frame: ids[0],
                frame_used: false,
                stages: ids[1..].try_into().unwrap(),
                stages_used: 0,
                pending: false,
            })
            .collect();
    }
    if log.frame_query_running {
        gl::EndQuery(TIME_ELAPSED);
        log.frame_query_running = false;
    }
    let slot = &mut log.slots[log.slot];
    gl::BeginQuery(TIME_ELAPSED, slot.stages[0]);
    slot.stages_used = 1;
    log.stage = Some(0);
}

/// Call between presenting stages: ends the current one and starts the next.
pub unsafe fn present_stage() {
    let mut state = STATE.lock().unwrap();
    let Some(log) = state.as_mut() else {
        return;
    };
    let Some(stage) = log.stage else {
        return;
    };
    if stage + 1 >= STAGES.len() {
        return;
    }
    gl::EndQuery(TIME_ELAPSED);
    let slot = &mut log.slots[log.slot];
    gl::BeginQuery(TIME_ELAPSED, slot.stages[stage + 1]);
    slot.stages_used = stage + 2;
    log.stage = Some(stage + 1);
}

/// Call at the end of presenting, with the presenting context still current.
/// `cpu_time` is how long presenting took on the CPU.
pub unsafe fn present_end(cpu_time: Duration) {
    let mut state = STATE.lock().unwrap();
    let Some(log) = state.as_mut() else {
        return;
    };
    if log.stage.take().is_none() {
        return;
    }
    gl::EndQuery(TIME_ELAPSED);
    log.slots[log.slot].pending = true;

    // Collect whatever results are ready.
    for slot in log.slots.iter_mut().filter(|slot| slot.pending) {
        let last = slot.stages[slot.stages_used - 1];
        let mut available = 0;
        gl::GetQueryObjectuiv(last, gl::QUERY_RESULT_AVAILABLE, &mut available);
        if available == 0 {
            continue;
        }
        if slot.frame_used {
            let mut ns = 0;
            gl::GetQueryObjectuiv(slot.frame, gl::QUERY_RESULT, &mut ns);
            log.gpu_frame_ns += ns as u64;
        }
        for (i, &query) in slot.stages[..slot.stages_used].iter().enumerate() {
            let mut ns = 0;
            gl::GetQueryObjectuiv(query, gl::QUERY_RESULT, &mut ns);
            log.gpu_stage_ns[i] += ns as u64;
        }
        log.gpu_samples += 1;
        slot.pending = false;
    }

    // The app's next frame goes in the next slot (dropping its old result if
    // it never arrived); its timing starts at its first draw.
    log.slot = (log.slot + 1) % log.slots.len();
    let slot = &mut log.slots[log.slot];
    slot.pending = false;
    slot.frame_used = false;

    let now = Instant::now();
    if let Some(last) = log.last_present {
        log.longest_frame = log.longest_frame.max(now - last);
    }
    log.last_present = Some(now);
    log.frames += 1;
    log.present_cpu += cpu_time;
    if now - log.second_start >= Duration::from_secs(1) {
        write_row(log, now);
    }
}

fn write_row(log: &mut PerfLog, now: Instant) {
    let seconds = (now - log.second_start).as_secs_f64();
    let frames = log.frames.max(1) as f64;
    let per_frame = |n: u64| n as f64 / frames;
    let gpu_samples = log.gpu_samples.max(1) as f64;
    let gpu_ms = |ns: u64| ns as f64 / gpu_samples / 1e6;
    let unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let stages: String = log
        .gpu_stage_ns
        .iter()
        .map(|&ns| format!(",{:.3}", gpu_ms(ns)))
        .collect();
    let c = &log.counters;
    let _ = writeln!(
        log.file,
        "{},{:.1},{:.1},{:.1},{:.1},{:.0},{:.1},{:.1},{:.1},{:.1},{:.1},{},{:.3},{:.3}{},{:.3},{}",
        unix_ms,
        (now - log.start).as_secs_f64(),
        log.frames as f64 / seconds,
        log.longest_frame.as_secs_f64() * 1000.0,
        per_frame(c.draws),
        per_frame(c.vertices),
        per_frame(c.texture_upload_bytes) / 1024.0,
        per_frame(c.buffer_upload_bytes) / 1024.0,
        per_frame(c.clears),
        per_frame(c.framebuffer_binds),
        per_frame(c.program_binds),
        c.read_pixels,
        gpu_ms(log.gpu_frame_ns),
        gpu_ms(log.gpu_stage_ns.iter().sum()),
        stages,
        log.present_cpu.as_secs_f64() * 1000.0 / frames,
        log.markers,
    );
    let _ = log.file.flush();
    log.second_start = now;
    log.frames = 0;
    log.longest_frame = Duration::ZERO;
    log.present_cpu = Duration::ZERO;
    log.counters = Counters::default();
    log.gpu_frame_ns = 0;
    log.gpu_stage_ns = [0; STAGES.len()];
    log.gpu_samples = 0;
    log.markers = 0;
}
