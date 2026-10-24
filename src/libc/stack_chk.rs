/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Stack protector support (`-fstack-protector`).
//!
//! Private Gridrunner fork, not part of upstream touchHLE. Compilers emit
//! code that copies `__stack_chk_guard` into a stack frame on entry and checks
//! it on exit, calling `__stack_chk_fail` if it was overwritten.

use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::Environment;

/// Arbitrary non-zero canary. It contains a zero byte so string overflows
/// can't easily reproduce it, like common C library implementations.
const GUARD_VALUE: u32 = 0x00a7_3c5e;

pub const CONSTANTS: ConstantExports = &[(
    "___stack_chk_guard",
    HostConstant::Custom(|env| env.mem.alloc_and_write(GUARD_VALUE).cast_void().cast_const()),
)];

fn __stack_chk_fail(_env: &mut Environment) {
    panic!("Guest stack smashing detected (__stack_chk_fail)");
}

pub const FUNCTIONS: FunctionExports = &[export_c_func!(__stack_chk_fail())];
