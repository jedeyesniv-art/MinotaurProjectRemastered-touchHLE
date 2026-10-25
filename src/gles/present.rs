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
    /// Regions of the frame moved into the right-hand margin, and their scale
    /// (see `Window::margin_pieces`).
    pub margin: Option<(Vec<[f32; 4]>, f32)>,
}

/// Private fork, `--sharp-scaling`: a framebuffer and texture, kept per
/// context, holding the frame blown up by a whole number with no filtering.
#[derive(Copy, Clone, Default)]
pub struct SharpScaler {
    framebuffer: u32,
    texture: u32,
    size: (u32, u32),
}

/// Private fork: `frame_size` is the size in pixels of the frame texture.
pub unsafe fn present_frame(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
    rotation_matrix: Matrix<2>,
    virtual_cursor_visible_at: Option<(f32, f32, bool)>,
    bezel: Option<Bezel>,
    frame_size: (u32, u32),
    sharp: Option<&mut SharpScaler>,
) {
    // While this is a generic utility, it is closely tied to
    // crate::frameworks::opengles::eagl::present_renderbuffer, which handles
    // backing up and restoring OpenGL ES state that this function might touch,
    // so these need to be updated in tandem.

    use gles11::types::*;

    gles.ClearColor(0.0, 0.0, 0.0, 1.0);
    gles.Clear(gles11::COLOR_BUFFER_BIT | gles11::DEPTH_BUFFER_BIT | gles11::STENCIL_BUFFER_BIT);
    if crate::perf_log::enabled() {
        crate::perf_log::present_stage();
    }
    gles.BindBuffer(gles11::ARRAY_BUFFER, 0);
    let vertices: [f32; 12] = [
        -1.0, -1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0,
    ];

    if let Some(bezel) = &bezel {
        draw_bezel(gles, bezel, &vertices);
    }

    if crate::perf_log::enabled() {
        crate::perf_log::present_stage();
    }

    let mut frame_texture: GLint = 0;
    gles.GetIntegerv(gles11::TEXTURE_BINDING_2D, &mut frame_texture);
    let frame_texture = frame_texture as GLuint;
    // How many screen pixels each frame pixel covers.
    let scale = ((viewport.2 * viewport.3) as f32 / (frame_size.0 * frame_size.1) as f32).sqrt();
    if let Some(sharp) = sharp {
        prescale_frame(gles, sharp, frame_texture, frame_size, scale, &vertices);
    }
    if crate::perf_log::enabled() {
        crate::perf_log::present_stage();
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
    // Private fork: rotate the texture co-ordinates about the texture's
    // centre, not its corner, so they stay within 0..1. Corner rotation only
    // worked because the frame texture repeats; the --sharp-scaling texture
    // clamps, which smeared its edge pixels across a landscape picture.
    let mut columns = *Matrix::<4>::from(&rotation_matrix).columns();
    let (cx, cy) = (
        0.5 * (columns[0][0] + columns[1][0]),
        0.5 * (columns[0][1] + columns[1][1]),
    );
    columns[3][0] = 0.5 - cx;
    columns[3][1] = 0.5 - cy;
    let matrix = Matrix::<4>::from_columns(columns);
    gles.MatrixMode(gles11::TEXTURE);
    gles.LoadMatrixf(matrix.columns().as_ptr() as *const _);
    gles.Enable(gles11::TEXTURE_2D);
    gles.DrawArrays(gles11::TRIANGLES, 0, 6);
    // clean this up so we don't need to worry about it in e.g. Core Animation
    gles.LoadIdentity();
    if crate::perf_log::enabled() {
        crate::perf_log::present_stage();
    }

    if let Some(Bezel {
        screen_size,
        margin: Some((pieces, margin_scale)),
        ..
    }) = &bezel
    {
        gles.BindTexture(gles11::TEXTURE_2D, frame_texture);
        let piece_scale = ((scale * *margin_scale).round() as u32).max(1);
        draw_margin_pieces(
            gles,
            viewport,
            *screen_size,
            pieces,
            frame_size,
            piece_scale,
            &vertices,
        );
    }

    // Display virtual cursor
    if let Some((x, y, pressed)) = virtual_cursor_visible_at {
        let (vx, vy, vw, vh) = viewport;
        let x = x - vx as f32;
        let y = y - vy as f32;

        gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
        gles.Disable(gles11::TEXTURE_2D);

        gles.Enable(gles11::BLEND);
        gles.BlendFunc(gles11::ONE, gles11::ONE_MINUS_SRC_ALPHA);

        // Private fork: white with a dark centre (filled when pressed), sized
        // to the picture, so it shows on dark games too.
        let radius = (vh as f32 / 768.0 * 12.0).max(10.0);
        let quad = |radius: f32| {
            let mut quad = vertices;
            for i in (0..quad.len()).step_by(2) {
                quad[i] = (quad[i] * radius + x) / (vw as f32 / 2.0) - 1.0;
                quad[i + 1] = 1.0 - (quad[i + 1] * radius + y) / (vh as f32 / 2.0);
            }
            quad
        };
        let outer = quad(radius);
        let inner = quad(radius * 0.5);
        gles.Color4f(0.9, 0.9, 0.9, 0.9);
        gles.VertexPointer(2, gles11::FLOAT, 0, outer.as_ptr() as *const GLvoid);
        gles.DrawArrays(gles11::TRIANGLES, 0, 6);
        let a = if pressed { 0.9 } else { 0.6 };
        gles.Color4f(0.0, 0.0, 0.0, a);
        gles.VertexPointer(2, gles11::FLOAT, 0, inner.as_ptr() as *const GLvoid);
        gles.DrawArrays(gles11::TRIANGLES, 0, 6);
    }
}

/// Private fork: upload a bezel picture to a new texture in the current
/// context, with mipmaps so it shrinks cheaply and cleanly on smaller screens.
/// The texture binding is left changed.
pub unsafe fn upload_bezel(gles: &mut dyn GLES, image: &crate::image::Image) -> u32 {
    let mut texture = 0;
    gles.GenTextures(1, &mut texture);
    gles.BindTexture(gles11::TEXTURE_2D, texture);
    gles.TexParameteri(gles11::TEXTURE_2D, gles11::GENERATE_MIPMAP, gles11::TRUE as _);
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
    gles.TexParameteri(
        gles11::TEXTURE_2D,
        gles11::TEXTURE_MIN_FILTER,
        gles11::LINEAR_MIPMAP_LINEAR as _,
    );
    gles.TexParameteri(gles11::TEXTURE_2D, gles11::TEXTURE_MAG_FILTER, gles11::LINEAR as _);
    texture
}

/// Private fork, `--sharp-scaling`: draw the frame into the scaler's texture
/// blown up by the largest whole number that doesn't exceed `scale` (at least
/// 1), with no filtering, and leave that texture bound and the window's
/// framebuffer current. Drawing it smoothly from there to the screen only
/// blends at pixel edges, so the picture stays sharp at any size.
unsafe fn prescale_frame(
    gles: &mut dyn GLES,
    sharp: &mut SharpScaler,
    frame_texture: u32,
    (frame_w, frame_h): (u32, u32),
    scale: f32,
    vertices: &[f32; 12],
) {
    use gles11::types::*;

    let factor = (scale.floor() as u32).max(1);
    let size = (frame_w * factor, frame_h * factor);
    if sharp.texture == 0 {
        gles.GenTextures(1, &mut sharp.texture);
        gles.GenFramebuffersOES(1, &mut sharp.framebuffer);
    }
    gles.BindTexture(gles11::TEXTURE_2D, sharp.texture);
    if sharp.size != size {
        gles.TexImage2D(
            gles11::TEXTURE_2D,
            0,
            gles11::RGB as _,
            size.0 as _,
            size.1 as _,
            0,
            gles11::RGB,
            gles11::UNSIGNED_BYTE,
            std::ptr::null(),
        );
        for param in [gles11::TEXTURE_MIN_FILTER, gles11::TEXTURE_MAG_FILTER] {
            gles.TexParameteri(gles11::TEXTURE_2D, param, gles11::LINEAR as _);
        }
        for param in [gles11::TEXTURE_WRAP_S, gles11::TEXTURE_WRAP_T] {
            gles.TexParameteri(gles11::TEXTURE_2D, param, gles11::CLAMP_TO_EDGE as _);
        }
        sharp.size = size;
    }
    gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, sharp.framebuffer);
    gles.FramebufferTexture2DOES(
        gles11::FRAMEBUFFER_OES,
        gles11::COLOR_ATTACHMENT0_OES,
        gles11::TEXTURE_2D,
        sharp.texture,
        0,
    );
    gles.Viewport(0, 0, size.0 as _, size.1 as _);

    gles.BindTexture(gles11::TEXTURE_2D, frame_texture);
    for param in [gles11::TEXTURE_MIN_FILTER, gles11::TEXTURE_MAG_FILTER] {
        gles.TexParameteri(gles11::TEXTURE_2D, param, gles11::NEAREST as _);
    }
    gles.MatrixMode(gles11::TEXTURE);
    gles.LoadIdentity();
    gles.EnableClientState(gles11::VERTEX_ARRAY);
    gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
    let tex_coords: [f32; 12] = [0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
    gles.TexCoordPointer(2, gles11::FLOAT, 0, tex_coords.as_ptr() as *const GLvoid);
    gles.Enable(gles11::TEXTURE_2D);
    gles.DrawArrays(gles11::TRIANGLES, 0, 6);

    gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, 0);
    gles.BindTexture(gles11::TEXTURE_2D, sharp.texture);
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

/// Private fork: copy regions of the frame (bound to `GL_TEXTURE_2D`, upright
/// portrait) into the margin right of the viewport at a whole-number `scale`,
/// stacked and centred there around 30% of the way down, then black them out
/// where they were.
unsafe fn draw_margin_pieces(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
    screen_size: (u32, u32),
    pieces: &[[f32; 4]],
    frame_size: (u32, u32),
    scale: u32,
    vertices: &[f32; 12],
) {
    use gles11::types::*;

    let (vx, vy, vw, vh) = viewport;
    let (screen_w, screen_h) = screen_size;
    let margin_x = vx + vw;
    if margin_x >= screen_w {
        return;
    }
    let margin_w = screen_w - margin_x;

    let sizes: Vec<(u32, u32)> = pieces
        .iter()
        .map(|&[_, _, w, h]| {
            (
                (w * frame_size.0 as f32).round() as u32 * scale,
                (h * frame_size.1 as f32).round() as u32 * scale,
            )
        })
        .collect();
    let gap = sizes.iter().map(|&(_, h)| h).max().unwrap_or(0) / 3;
    let total_h = sizes.iter().map(|&(_, h)| h).sum::<u32>() + gap * (sizes.len() as u32 - 1);
    let mut top = (screen_h as f32 * 0.30) as i32 - total_h as i32 / 2;

    gles.EnableClientState(gles11::VERTEX_ARRAY);
    gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
    gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
    gles.Enable(gles11::TEXTURE_2D);
    for (&[u, v, w, h], &(dest_w, dest_h)) in pieces.iter().zip(&sizes) {
        // The frame's first row is its bottom.
        let (u0, u1, v_top, v_bottom) = (u, u + w, 1.0 - v, 1.0 - v - h);
        let tex_coords: [f32; 12] = [
            u0, v_bottom, u0, v_top, u1, v_bottom, u1, v_bottom, u0, v_top, u1, v_top,
        ];
        gles.TexCoordPointer(2, gles11::FLOAT, 0, tex_coords.as_ptr() as *const GLvoid);
        let x = margin_x as i32 + (margin_w as i32 - dest_w as i32) / 2;
        let y = screen_h as i32 - top - dest_h as i32;
        gles.Viewport(x, y, dest_w as _, dest_h as _);
        gles.DrawArrays(gles11::TRIANGLES, 0, 6);
        top += (dest_h + gap) as i32;
    }

    gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
    gles.Disable(gles11::TEXTURE_2D);
    gles.Color4f(0.0, 0.0, 0.0, 1.0);
    for &[u, v, w, h] in pieces {
        let x0 = vx as f32 + u * vw as f32;
        let x1 = vx as f32 + (u + w) * vw as f32;
        let y0 = vy as f32 + (1.0 - v - h) * vh as f32;
        let y1 = vy as f32 + (1.0 - v) * vh as f32;
        let (x0, y0) = (x0.floor() as i32, y0.floor() as i32);
        let (x1, y1) = (x1.ceil() as i32, y1.ceil() as i32);
        gles.Viewport(x0, y0, x1 - x0, y1 - y0);
        gles.DrawArrays(gles11::TRIANGLES, 0, 6);
    }
    gles.Color4f(1.0, 1.0, 1.0, 1.0);
    gles.Enable(gles11::TEXTURE_2D);
    gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
    gles.Viewport(vx as _, vy as _, vw as _, vh as _);
}
