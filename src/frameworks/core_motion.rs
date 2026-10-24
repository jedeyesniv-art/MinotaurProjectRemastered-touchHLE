/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The Core Motion framework.

use crate::abi::GuestArg;
use crate::dyld::HostDylib;
use crate::frameworks::foundation::NSTimeInterval;
use crate::impl_GuestRet_for_large_struct;
use crate::mem::SafeRead;
use crate::objc::{id, objc_classes, ClassExports, TrivialHostObject};

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/CoreMotion.framework/CoreMotion",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[],
    function_exports: &[],
};

#[derive(Default)]
pub struct State {
    /// The single `CMAccelerometerData` instance, see `accelerometerData`.
    accelerometer_data: Option<id>,
}

/// `CMAcceleration`: acceleration in g-force units.
#[derive(Copy, Clone, Debug, Default)]
#[repr(C, packed)]
pub struct CMAcceleration {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}
unsafe impl SafeRead for CMAcceleration {}
impl_GuestRet_for_large_struct!(CMAcceleration);
impl GuestArg for CMAcceleration {
    const REG_COUNT: usize = 6;
    fn from_regs(regs: &[u32]) -> Self {
        let f = |i: usize| f64::from_bits(u64::from(regs[i]) | (u64::from(regs[i + 1]) << 32));
        CMAcceleration {
            x: f(0),
            y: f(2),
            z: f(4),
        }
    }
    fn to_regs(self, regs: &mut [u32]) {
        for (i, v) in [self.x, self.y, self.z].into_iter().enumerate() {
            let bits = v.to_bits();
            regs[i * 2] = bits as u32;
            regs[i * 2 + 1] = (bits >> 32) as u32;
        }
    }
}

/// Private Gridrunner fork: CoreMotion always reports the device standing
/// upright in portrait, regardless of any game controller. Apps that only use
/// the accelerometer to pick a display orientation then stay the right way up,
/// and the analog sticks remain free for other mappings (e.g.
/// `--stick-to-touch`).
const UPRIGHT_PORTRAIT: CMAcceleration = CMAcceleration {
    x: 0.0,
    y: -1.0,
    z: 0.0,
};

const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation CMMotionManager: NSObject

- (bool)isGyroAvailable {
    // It could make sense to implement gyroscope support at least for Android.
    log!("TODO: [(CMMotionManager *){:?} isGyroAvailable] -> false", this);
    false
}
- (bool)isDeviceMotionAvailable {
    log!("TODO: [(CMMotionManager *){:?} isDeviceMotionAvailable] -> false", this);
    // According to docs, this is functionally equivalent to `isGyroAvailable`
    // method. (All devices have accelerometer, but only some do have gyro).
    false
}
- (bool)isAccelerometerAvailable {
    // According to https://developer.apple.com/documentation/coremotion/getting-raw-accelerometer-events?language=objc,
    // every iOS device has an accelerometer, but on real hardware this method
    // can still return false if the device isn't ready to produce data yet.
    // Here we always return true since we don't model that readiness state.
    true
}

- (())setAccelerometerUpdateInterval:(NSTimeInterval)interval {
    log_dbg!("[(CMMotionManager *){:?} setAccelerometerUpdateInterval:{}] (ignored)", this, interval);
}
- (bool)isAccelerometerActive {
    true
}
- (())startAccelerometerUpdates {
    log_dbg!("[(CMMotionManager *){:?} startAccelerometerUpdates]", this);
}
- (())stopAccelerometerUpdates {
    log_dbg!("[(CMMotionManager *){:?} stopAccelerometerUpdates]", this);
}
- (id)accelerometerData {
    if let Some(data) = env.framework_state.core_motion.accelerometer_data {
        return data;
    }
    let class = env.objc.get_known_class("CMAccelerometerData", &mut env.mem);
    let data = env.objc.alloc_static_object(class, Box::new(TrivialHostObject), &mut env.mem);
    env.framework_state.core_motion.accelerometer_data = Some(data);
    data
}

@end

@implementation CMAccelerometerData: NSObject

- (CMAcceleration)acceleration {
    UPRIGHT_PORTRAIT
}
- (NSTimeInterval)timestamp {
    0.0
}

@end

};
