/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! A very small subset of Grand Central Dispatch.
//!
//! Private Gridrunner fork, not part of upstream touchHLE. Only the main
//! queue exists, and `dispatch_async` runs the block immediately on the
//! calling thread, which is adequate for apps that use it to hop back to the
//! main thread from callbacks that are already on the main thread.

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::mem::{ConstVoidPtr, MutVoidPtr, Ptr};
use crate::Environment;

pub const CONSTANTS: ConstantExports = &[(
    "__dispatch_main_q",
    // Opaque queue object: apps only take its address.
    HostConstant::Custom(|env| env.mem.alloc(64).cast_const()),
)];

/// Call a block (see the Clang Block ABI: `invoke` is at offset 12 on 32-bit
/// targets and takes the block itself as its first argument).
fn invoke_block(env: &mut Environment, block: ConstVoidPtr) {
    let invoke_ptr: Ptr<GuestFunction, false> = Ptr::from_bits(block.to_bits() + 12);
    let invoke: GuestFunction = env.mem.read(invoke_ptr);
    () = invoke.call_from_host(env, (block,));
}

fn dispatch_async(env: &mut Environment, queue: MutVoidPtr, block: ConstVoidPtr) {
    log_dbg!("dispatch_async({:?}, {:?}): running block synchronously", queue, block);
    invoke_block(env, block);
}

pub const FUNCTIONS: FunctionExports = &[export_c_func!(dispatch_async(_, _))];
