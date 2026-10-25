/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CAEAGLLayer`.

use super::ca_layer::CALayerHostObject;
use crate::frameworks::core_graphics::{CGPoint, CGRect};
use crate::objc::{id, msg, msg_class, nil, objc_classes, Class, ClassExports};
use crate::Environment;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation CAEAGLLayer: CALayer

// EAGLDrawable implementation (the only one)

- (id)drawableProperties {
    // FIXME: do we need to return an empty dictionary rather than nil?
    env.objc.borrow::<CALayerHostObject>(this).drawable_properties
}

- (())setDrawableProperties:(id)props { // NSDictionary<NSString*, id>*
    let props: id = msg![env; props copy];
    env.objc.borrow_mut::<CALayerHostObject>(this).drawable_properties = props;
}

@end

};

/// If there is an opaque `CAEAGLLayer` that covers the entire screen, this
/// returns a pointer to it. Otherwise, it returns [nil].
///
/// To avoid a state management nightmare, we want to have an internal OpenGL ES
/// context for compositing, separate from any OpenGL ES contexts the app uses
/// for its rendering. When we have a `CAEAGLLayer` though, we need to transfer
/// a rendered frame from the app's context to the compositor's context, and
/// unfortunately the most practical way to do this is `glReadPixels()`, which
/// is highly inefficient. To make things efficient, then, we have a shortcut:
/// if the result of composition would be identical to the rendered frame, i.e.
/// there's a single full-screen layer, we skip transferring between contexts
/// and present it directly from the app's context. This function is used to
/// determine when that will happen.
pub fn find_fullscreen_eagl_layer(env: &mut Environment) -> id {
    find_fullscreen_eagl_layer_and_crop(env).0
}

/// Private fork: like [find_fullscreen_eagl_layer], but the layer only has to
/// cover the screen, not match it exactly (Gridrunner's window is 768x1029 on
/// a 768x1024 screen). Also returns the part of the layer that is on screen,
/// as (x, y, width, height) fractions of its bounds, top-left origin.
pub fn find_fullscreen_eagl_layer_and_crop(env: &mut Environment) -> (id, [f32; 4]) {
    let full = [0.0, 0.0, 1.0, 1.0];
    if env.options.force_composition {
        return (nil, full);
    }

    let windows = env.framework_state.uikit.ui_view.ui_window.windows.clone();
    // Assumes the windows in the list are ordered back-to-front.
    // TODO: this may not be correct once we support windowLevel.
    let Some(top_window) = windows
        .into_iter()
        .rev()
        .find(|&window| !msg![env; window isHidden])
    else {
        return (nil, full);
    };

    let screen_bounds: CGRect = {
        let screen: id = msg_class![env; UIScreen mainScreen];
        msg![env; screen bounds]
    };

    let mut layer: id = msg![env; top_window layer];
    // Top-left corner of the current layer in screen co-ordinates.
    let mut origin = CGPoint { x: 0.0, y: 0.0 };

    // Descend through the hierarchy, looking only at the last layer in each
    // list of children, since that should be the one on top.
    // TODO: this is not correct once we support zPosition.
    loop {
        assert!(layer != nil);

        let layer_host_obj: &CALayerHostObject = env.objc.borrow(layer);

        // This is stricter than it should be. In theory we should accumulate
        // the transforms, but real apps probably only use this common case.
        let size = layer_host_obj.bounds.size;
        origin = CGPoint {
            x: origin.x + layer_host_obj.position.x - size.width * layer_host_obj.anchor_point.x,
            y: origin.y + layer_host_obj.position.y - size.height * layer_host_obj.anchor_point.y,
        };
        if origin.x > 0.0
            || origin.y > 0.0
            || origin.x + size.width < screen_bounds.size.width
            || origin.y + size.height < screen_bounds.size.height
            || layer_host_obj.bounds.origin != (CGPoint { x: 0.0, y: 0.0 })
            || layer_host_obj.hidden
            || layer_host_obj.opacity != 1.0
            // TODO: support affine transforms that result in a full-screen
            //       layer (typical example is 90° rotation).
            || !layer_host_obj.affine_transform.is_identity()
        {
            return (nil, full);
        }

        // Private fork: skip over layers that draw nothing (Gridrunner puts
        // an empty view and a zero-size iCade reader view above its game).
        let sublayers = layer_host_obj.sublayers.clone();
        if let Some(&next) = sublayers.iter().rev().find(|&&l| !draws_nothing(env, l)) {
            layer = next;
        } else {
            break;
        }
    }

    let layer_host_obj = env.objc.borrow::<CALayerHostObject>(layer);
    if !layer_host_obj.opaque {
        return (nil, full);
    }
    let size = layer_host_obj.bounds.size;
    let crop = [
        -origin.x / size.width,
        -origin.y / size.height,
        screen_bounds.size.width / size.width,
        screen_bounds.size.height / size.height,
    ];

    let ca_eagl_layer_class: Class = msg_class![env; CAEAGLLayer class];
    if !msg![env; layer isKindOfClass:ca_eagl_layer_class] {
        return (nil, full);
    }

    (layer, crop)
}

/// Private fork: whether a layer and its sublayers are certainly invisible:
/// hidden, transparent, zero-size, or with no background colour, contents or
/// drawing of their own.
fn draws_nothing(env: &Environment, layer: id) -> bool {
    let host_obj = env.objc.borrow::<CALayerHostObject>(layer);
    let size = host_obj.bounds.size;
    let empty = size.width * size.height == 0.0
        || (host_obj.background_color.is_none()
            && host_obj.contents == nil
            && host_obj.cg_context.is_none()
            && host_obj.presented_pixels.is_none());
    host_obj.hidden
        || host_obj.opacity == 0.0
        || (empty && host_obj.sublayers.iter().all(|&l| draws_nothing(env, l)))
}

/// For use by `EAGLContext` when presenting to a `CAEAGLLayer`:
/// [std::mem::take]s the buffer used to hold the pixels. It should be passed
/// back to [present_pixels] once it has been filled.
pub fn get_pixels_vec_for_presenting(env: &mut Environment, layer: id) -> Vec<u8> {
    env.objc
        .borrow_mut::<CALayerHostObject>(layer)
        .presented_pixels
        .take()
        .map(|(vec, _width, _height)| vec)
        .unwrap_or_default()
}

/// For use by `EAGLContext` when presenting to a `CAEAGLLayer`: provide the new
/// frame rendered by the app, so it can be used when compositing. The buffer
/// should have been obtained with [get_pixels_vec_for_presenting] before
/// filling. The data must be in RGBA8 format.
pub fn present_pixels(env: &mut Environment, layer: id, pixels: Vec<u8>, width: u32, height: u32) {
    let host_obj = env.objc.borrow_mut::<CALayerHostObject>(layer);
    host_obj.presented_pixels = Some((pixels, width, height));
    host_obj.gles_texture_is_up_to_date = false;
}
