/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Utilities for presenting frames to the window using an abstract OpenGL ES
//! implementation.

use super::gles11_raw as gles11; // constants and types only
use super::GLES;
use crate::matrix::Matrix;
use std::time::{Duration, Instant};

pub struct FpsCounter {
    time: std::time::Instant,
    frames: u32,
}
impl FpsCounter {
    pub fn start() -> Self {
        FpsCounter {
            time: Instant::now(),
            frames: 0,
        }
    }

    pub fn count_frame(&mut self, label: std::fmt::Arguments<'_>) {
        self.frames += 1;
        let now = Instant::now();
        let duration = now - self.time;
        if duration >= Duration::from_secs(1) {
            self.time = now;
            echo!(
                "touchHLE: {} FPS: {:.2}",
                label,
                std::mem::take(&mut self.frames) as f32 / duration.as_secs_f32()
            );
        }
    }
}

/// Present the the latest frame (e.g. the app's splash screen or rendering
/// output), provided as a texture bound to `GL_TEXTURE_2D`, by drawing it on
/// the window. It may be rotated, scaled and/or letterboxed as necessary. The
/// virtual cursor is also drawn if it should be currently visible.
///
/// The provided context must be current.
/// Private fork: a picture drawn across the whole screen behind the frame.
pub struct Bezel {
    /// Texture with the picture's pixels, top row first.
    pub texture: u32,
    pub image_size: (u32, u32),
    pub screen_size: (u32, u32),
}

pub unsafe fn present_frame(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
    rotation_matrix: Matrix<2>,
    virtual_cursor_visible_at: Option<(f32, f32, bool)>,
    bezel: Option<Bezel>,
) {
    // While this is a generic utility, it is closely tied to
    // crate::frameworks::opengles::eagl::present_renderbuffer, which handles
    // backing up and restoring OpenGL ES state that this function might touch,
    // so these need to be updated in tandem.

    use gles11::types::*;

    gles.ClearColor(0.0, 0.0, 0.0, 1.0);
    gles.Clear(gles11::COLOR_BUFFER_BIT | gles11::DEPTH_BUFFER_BIT | gles11::STENCIL_BUFFER_BIT);
    gles.BindBuffer(gles11::ARRAY_BUFFER, 0);
    let vertices: [f32; 12] = [
        -1.0, -1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0,
    ];

    if let Some(bezel) = bezel {
        draw_bezel(gles, &bezel, &vertices);
    }

    // Draw the quad
    gles.Viewport(
        viewport.0 as _,
        viewport.1 as _,
        viewport.2 as _,
        viewport.3 as _,
    );
    gles.EnableClientState(gles11::VERTEX_ARRAY);
    gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
    let tex_coords: [f32; 12] = [0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
    gles.TexCoordPointer(2, gles11::FLOAT, 0, tex_coords.as_ptr() as *const GLvoid);
    let matrix = Matrix::<4>::from(&rotation_matrix);
    gles.MatrixMode(gles11::TEXTURE);
    gles.LoadMatrixf(matrix.columns().as_ptr() as *const _);
    gles.Enable(gles11::TEXTURE_2D);
    gles.DrawArrays(gles11::TRIANGLES, 0, 6);
    // clean this up so we don't need to worry about it in e.g. Core Animation
    gles.LoadIdentity();

    // Display virtual cursor
    if let Some((x, y, pressed)) = virtual_cursor_visible_at {
        let (vx, vy, vw, vh) = viewport;
        let x = x - vx as f32;
        let y = y - vy as f32;

        gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
        gles.Disable(gles11::TEXTURE_2D);

        gles.Enable(gles11::BLEND);
        gles.BlendFunc(gles11::ONE, gles11::ONE_MINUS_SRC_ALPHA);
        gles.Color4f(0.0, 0.0, 0.0, if pressed { 2.0 / 3.0 } else { 1.0 / 3.0 });

        let radius = 10.0;

        let mut vertices = vertices;
        for i in (0..vertices.len()).step_by(2) {
            vertices[i] = (vertices[i] * radius + x) / (vw as f32 / 2.0) - 1.0;
            vertices[i + 1] = 1.0 - (vertices[i + 1] * radius + y) / (vh as f32 / 2.0);
        }
        gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
        gles.DrawArrays(gles11::TRIANGLES, 0, 6);
    }
}

/// Private fork: upload a bezel picture to a new texture in the current
/// context. The texture binding is left changed.
pub unsafe fn upload_bezel(gles: &mut dyn GLES, image: &crate::image::Image) -> u32 {
    let mut texture = 0;
    gles.GenTextures(1, &mut texture);
    gles.BindTexture(gles11::TEXTURE_2D, texture);
    let (width, height) = image.dimensions();
    gles.TexImage2D(
        gles11::TEXTURE_2D,
        0,
        gles11::RGBA as _,
        width as _,
        height as _,
        0,
        gles11::RGBA,
        gles11::UNSIGNED_BYTE,
        image.pixels().as_ptr() as *const _,
    );
    for param in [gles11::TEXTURE_MIN_FILTER, gles11::TEXTURE_MAG_FILTER] {
        gles.TexParameteri(gles11::TEXTURE_2D, param, gles11::LINEAR as _);
    }
    texture
}

/// Draw the bezel over the whole screen, cropped to fill it, leaving the
/// frame's texture bound afterwards.
unsafe fn draw_bezel(gles: &mut dyn GLES, bezel: &Bezel, vertices: &[f32; 12]) {
    use gles11::types::*;

    let mut frame_texture: GLint = 0;
    gles.GetIntegerv(gles11::TEXTURE_BINDING_2D, &mut frame_texture);

    let (screen_w, screen_h) = bezel.screen_size;
    gles.Viewport(0, 0, screen_w as _, screen_h as _);

    let image_aspect = bezel.image_size.0 as f32 / bezel.image_size.1 as f32;
    let screen_aspect = screen_w as f32 / screen_h as f32;
    let (du, dv) = if image_aspect > screen_aspect {
        ((1.0 - screen_aspect / image_aspect) / 2.0, 0.0)
    } else {
        (0.0, (1.0 - image_aspect / screen_aspect) / 2.0)
    };
    let (u0, u1, v_top, v_bottom) = (du, 1.0 - du, dv, 1.0 - dv);
    // Same corners as `vertices`; the image's first row is its top.
    let tex_coords: [f32; 12] = [
        u0, v_bottom, u0, v_top, u1, v_bottom, u1, v_bottom, u0, v_top, u1, v_top,
    ];

    gles.BindTexture(gles11::TEXTURE_2D, bezel.texture);
    gles.MatrixMode(gles11::TEXTURE);
    gles.LoadIdentity();
    gles.EnableClientState(gles11::VERTEX_ARRAY);
    gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
    gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
    gles.TexCoordPointer(2, gles11::FLOAT, 0, tex_coords.as_ptr() as *const GLvoid);
    gles.Enable(gles11::TEXTURE_2D);
    gles.DrawArrays(gles11::TRIANGLES, 0, 6);

    gles.BindTexture(gles11::TEXTURE_2D, frame_texture as GLuint);
}
