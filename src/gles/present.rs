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

/// Private fork, `--ambient-bars`: a small framebuffer and texture, kept per
/// context, holding a smoothed, shrunken copy of the picture (as displayed),
/// which is stretched across the screen behind it to fill the bars.
#[derive(Copy, Clone, Default)]
pub struct AmbientGlow {
    framebuffer: u32,
    texture: u32,
    size: (u32, u32),
    filled: bool,
    /// The shader that draws it (0 = not built yet, u32::MAX = failed, so the
    /// plain dimmed glow is drawn instead), and its uniforms.
    program: u32,
    uniforms: [i32; 4],
}

/// Private fork, `--edge-bars`: the colours the bars are shown in (left,
/// right), easing towards the sampled ones, kept per context.
#[derive(Copy, Clone, Default)]
pub struct EdgeBars {
    colours: [[f32; 3]; 2],
    filled: bool,
}

/// Private fork: what [present_frame] needs to draw the `--edge-bars`.
pub struct EdgeBarsArgs<'a> {
    pub state: &'a mut EdgeBars,
    /// The sampled colours of the picture's left and right edges.
    pub target: [[f32; 3]; 2],
    pub pattern: Option<EdgePattern>,
}

/// Private fork: the `--edge-bars-texture` pattern.
#[derive(Copy, Clone)]
pub struct EdgePattern {
    pub texture: u32,
    pub size: (u32, u32),
    /// Screen pixels per texel.
    pub texel: f32,
    /// The pattern's average colour (0-1).
    pub average: [f32; 3],
    /// How far (texels) the picture's edges have scrolled up.
    pub scroll: f32,
}

/// Private fork, `--starfield-bars`: falling stars on black in the bars, like
/// Goat Up's own background, kept per context. Measured on the game's menu
/// and demo (in screen points, the picture being 1024 high): about 33 stars
/// per 768x1024, soft dots 3-11 high and a little taller than wide, dim
/// (40-55% at their brightest), in saturated colours; each falls straight
/// down at its own steady speed, 37-320 points a second, whatever the game is
/// doing (its screen doesn't scroll).
#[derive(Default)]
pub struct Starfield {
    /// Across both bars: x from 0 to twice a bar's width (the right bar's
    /// stars are the ones past one width).
    stars: Vec<Star>,
    /// `--starfield-bars=warp`.
    warp_stars: Vec<WarpStar>,
    rng: u32,
    last_frame: Option<std::time::Instant>,
    /// The soft dot (0 = not made yet).
    texture: u32,
}

struct Star {
    x: f32,
    y: f32,
    /// Points a second.
    speed: f32,
    /// Width in points (the height is `STAR_TALLNESS` times it).
    size: f32,
    /// Colour times brightness.
    colour: [f32; 3],
}

/// Stars per square point.
const STAR_DENSITY: f32 = 33.0 / (768.0 * 1024.0);
const STAR_TALLNESS: f32 = 1.3;
const STAR_COLOURS: [[f32; 3]; 9] = [
    [1.0, 0.0, 0.0],
    [1.0, 0.5, 0.0],
    [1.0, 1.0, 0.0],
    [0.5, 1.0, 0.0],
    [0.0, 1.0, 0.5],
    [0.0, 1.0, 1.0],
    [0.0, 0.5, 1.0],
    [0.0, 0.0, 1.0],
    [0.95, 0.98, 1.0],
];

/// Private fork, `--starfield-bars=warp`: a star of Goat Up 2's background,
/// in 3D around the picture's centre. Measured by tracking the game's stars
/// (game select, 20 frames a second): they fly straight out from the
/// picture's centre with true perspective (1/r falls linearly in time on
/// each track), appear about 14 s before they'd reach the viewer, spread
/// over about +-1400 points-seconds sideways, white, about 42 in the
/// picture at once, growing from 3.7 to 12 points and from 63% to full
/// brightness as they come closer. This model gives the same speed at each
/// distance from the centre (e.g. 2.4, 4.8 and 7.6 points a frame at 250,
/// 350 and 450 points; the game 2.4, 4.5 and 7.4) and the same spread.
struct WarpStar {
    /// Sideways, in points times seconds: on screen it's `x / z` points from
    /// the picture's centre.
    x: f32,
    y: f32,
    /// Seconds until it would reach the viewer.
    z: f32,
}

/// The game's density (45, about 42 in the picture) left the bars sparse
/// (about 2.4 stars between them); the user asked for half as many again,
/// twice (about 5.5).
const WARP_STARS: usize = 102;
const WARP_FAR: f32 = 14.0;
const WARP_SPREAD: f32 = 1400.0;

impl Starfield {
    /// 0 to 1 (xorshift).
    fn random(&mut self) -> f32 {
        if self.rng == 0 {
            self.rng = 0x9E37_79B9;
        }
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1 << 24) as f32
    }

    /// A new warp star, far away (or anywhere on the way, to start).
    fn new_warp_star(&mut self, anywhere: bool) -> WarpStar {
        let x = (self.random() * 2.0 - 1.0) * WARP_SPREAD;
        let y = (self.random() * 2.0 - 1.0) * WARP_SPREAD;
        let z = if anywhere { 0.1 + self.random() * (WARP_FAR - 0.1) } else { WARP_FAR };
        WarpStar { x, y, z }
    }

    /// A new star somewhere across `width` points, at height `y`.
    fn new_star(&mut self, width: f32, y: f32) -> Star {
        let x = self.random() * width;
        let speed = 37.0 + 283.0 * self.random().powi(2);
        let size = 5.0 + 9.0 * self.random();
        let brightness = 0.2 + 0.35 * self.random();
        let colour = STAR_COLOURS[(self.random() * STAR_COLOURS.len() as f32) as usize % STAR_COLOURS.len()];
        Star { x, y, speed, size, colour: colour.map(|c| c * brightness) }
    }
}

/// Private fork: what [present_frame] needs to draw the `--starfield-bars`.
pub struct StarfieldArgs<'a> {
    pub state: &'a mut Starfield,
    /// The picture's height in screen points.
    pub points_high: u32,
    /// `--starfield-bars=warp`.
    pub warp: bool,
}

const GLOW_VERTEX_SHADER: &str = "
attribute vec2 a_pos;
varying vec2 v_uv;
void main() {
    v_uv = a_pos * 0.5 + 0.5;
    gl_Position = vec4(a_pos, 0.0, 1.0);
}
";

/// Bright and saturated beside the picture, fading to the plain dimmed glow
/// at the far edge of the bars.
const GLOW_FRAGMENT_SHADER: &str = "
precision mediump float;
uniform sampler2D u_glow;
uniform vec4 u_picture; // the picture's rectangle, 0-1 across the screen
uniform vec4 u_params;  // near brightness, far brightness, near saturation, fade distance
uniform vec4 u_screen;  // screen width / height
varying vec2 v_uv;
void main() {
    vec3 c = texture2D(u_glow, v_uv).rgb;
    vec2 d = max(vec2(0.0), max(u_picture.xy - v_uv, v_uv - u_picture.zw));
    d.x *= u_screen.x;
    float t = smoothstep(0.0, 1.0, clamp(length(d) / u_params.w, 0.0, 1.0));
    float grey = dot(c, vec3(0.299, 0.587, 0.114));
    c = clamp(mix(vec3(grey), c, mix(u_params.z, 1.0, t)), 0.0, 1.0);
    gl_FragColor = vec4(clamp(c * mix(u_params.x, u_params.y, t), 0.0, 1.0), 1.0);
}
";

unsafe fn build_glow_program(gles: &mut dyn GLES, glow: &mut AmbientGlow) {
    let names = [c"u_glow", c"u_picture", c"u_params", c"u_screen"];
    let built = build_program(gles, GLOW_VERTEX_SHADER, GLOW_FRAGMENT_SHADER, &names, &mut glow.uniforms);
    glow.program = built.unwrap_or_else(|what| {
        log!("--ambient-bars: glow shader didn't {what}, using the plain glow");
        u32::MAX
    });
}

/// Private fork: compile and link a shader program (attributes `a_pos` = 0,
/// `a_uv` = 1) and look up its uniforms. On failure, says what failed.
unsafe fn build_program(
    gles: &mut dyn GLES,
    vertex_source: &str,
    fragment_source: &str,
    uniform_names: &[&std::ffi::CStr],
    uniforms: &mut [i32],
) -> Result<u32, &'static str> {
    use gles11::types::*;
    const COMPILE_STATUS: GLenum = 0x8B81;
    const LINK_STATUS: GLenum = 0x8B82;
    const VERTEX_SHADER: GLenum = 0x8B31;
    const FRAGMENT_SHADER: GLenum = 0x8B30;

    let mut shaders = [0; 2];
    for (shader, (kind, source)) in shaders
        .iter_mut()
        .zip([(VERTEX_SHADER, vertex_source), (FRAGMENT_SHADER, fragment_source)])
    {
        *shader = gles.CreateShader(kind);
        gles.ShaderSource(*shader, source);
        gles.CompileShader(*shader);
        let mut ok = 0;
        gles.GetShaderiv(*shader, COMPILE_STATUS, &mut ok);
        if ok == 0 {
            return Err("compile");
        }
    }
    let program = gles.CreateProgram();
    gles.AttachShader(program, shaders[0]);
    gles.AttachShader(program, shaders[1]);
    gles.BindAttribLocation(program, 0, c"a_pos".as_ptr());
    gles.BindAttribLocation(program, 1, c"a_uv".as_ptr());
    gles.LinkProgram(program);
    let mut ok = 0;
    gles.GetProgramiv(program, LINK_STATUS, &mut ok);
    if ok == 0 {
        return Err("link");
    }
    for (uniform, name) in uniforms.iter_mut().zip(uniform_names) {
        *uniform = gles.GetUniformLocation(program, name.as_ptr());
    }
    Ok(program)
}

/// Private fork: what [present_frame] needs to draw the ambient glow.
pub struct Ambient<'a> {
    pub glow: &'a mut AmbientGlow,
    /// How bright the glow is (0-1).
    pub brightness: f32,
    /// Seconds for the glow to catch up with a change in the picture.
    pub smoothing: f32,
    /// Brightness right beside the picture, fading to `brightness`.
    pub near_brightness: f32,
    /// Saturation right beside the picture (1 = as is), fading to 1.
    pub near_saturation: f32,
    /// The window's size in pixels.
    pub screen_size: (u32, u32),
}

/// Private fork: `frame_size` is the size in pixels of the frame texture;
/// `trim_edges` (`--trim-edges`) is how many screen pixels to blacken at the
/// picture's left and right edges when there are bars beside it.
#[allow(clippy::too_many_arguments)]
pub unsafe fn present_frame(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
    rotation_matrix: Matrix<2>,
    virtual_cursor_visible_at: Option<(f32, f32, bool)>,
    bezel: Option<Bezel>,
    frame_size: (u32, u32),
    sharp: Option<&mut SharpScaler>,
    ambient: Option<Ambient<'_>>,
    edge_bars: Option<EdgeBarsArgs<'_>>,
    starfield: Option<StarfieldArgs<'_>>,
    trim_edges: u32,
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
    if let Some(ambient) = ambient {
        if bezel.is_none() {
            draw_ambient_glow(gles, ambient, frame_texture, viewport, &rotation_matrix, &vertices);
            gles.BindTexture(gles11::TEXTURE_2D, frame_texture);
        }
    }
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
    let matrix = centred_rotation(&rotation_matrix);
    gles.MatrixMode(gles11::TEXTURE);
    gles.LoadMatrixf(matrix.columns().as_ptr() as *const _);
    gles.Enable(gles11::TEXTURE_2D);
    gles.DrawArrays(gles11::TRIANGLES, 0, 6);
    // clean this up so we don't need to worry about it in e.g. Core Animation
    gles.LoadIdentity();
    if trim_edges > 0 && viewport.0 > 0 {
        // Private fork, --trim-edges: some games leave a line of garbage in
        // their frame's outermost columns (Goat Up 2); paint them black, the
        // bars' colour.
        let trim = trim_edges.min(viewport.2 / 2);
        gles.Disable(gles11::TEXTURE_2D);
        gles.Color4f(0.0, 0.0, 0.0, 1.0);
        for x in [viewport.0, viewport.0 + viewport.2 - trim] {
            gles.Viewport(x as _, viewport.1 as _, trim as _, viewport.3 as _);
            gles.DrawArrays(gles11::TRIANGLES, 0, 6);
        }
        gles.Color4f(1.0, 1.0, 1.0, 1.0);
        gles.Enable(gles11::TEXTURE_2D);
        gles.Viewport(viewport.0 as _, viewport.1 as _, viewport.2 as _, viewport.3 as _);
    }
    if let Some(edge_bars) = edge_bars {
        draw_edge_bars(gles, edge_bars, viewport, &vertices);
    }
    if let Some(starfield) = starfield {
        draw_starfield(gles, starfield, viewport, &vertices);
    }
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

/// Private fork: the texture matrix that turns the frame for display. It
/// rotates the texture co-ordinates about the texture's centre, not its
/// corner, so they stay within 0..1. Corner rotation only worked because the
/// frame texture repeats; the --sharp-scaling texture clamps, which smeared
/// its edge pixels across a landscape picture.
fn centred_rotation(rotation_matrix: &Matrix<2>) -> Matrix<4> {
    let mut columns = *Matrix::<4>::from(rotation_matrix).columns();
    let (cx, cy) = (
        0.5 * (columns[0][0] + columns[1][0]),
        0.5 * (columns[0][1] + columns[1][1]),
    );
    columns[3][0] = 0.5 - cx;
    columns[3][1] = 0.5 - cy;
    Matrix::<4>::from_columns(columns)
}

/// Private fork, `--ambient-bars`: blend this frame (turned for display and
/// shrunk to a few dozen pixels, sampled four times) into the glow texture,
/// then stretch the glow across the whole screen, dimmed. The picture is
/// drawn over it afterwards, so only the bars show it: each bar takes the
/// colours from its own side of the picture.
unsafe fn draw_ambient_glow(
    gles: &mut dyn GLES,
    ambient: Ambient<'_>,
    frame_texture: u32,
    viewport: (u32, u32, u32, u32),
    rotation_matrix: &Matrix<2>,
    vertices: &[f32; 12],
) {
    use gles11::types::*;

    let Ambient { glow, brightness, smoothing, near_brightness, near_saturation, screen_size } =
        ambient;
    // The glow has the picture's shape, 32 pixels on its long side.
    let (vw, vh) = (viewport.2.max(1) as f32, viewport.3.max(1) as f32);
    let size = if vw >= vh {
        (32, ((32.0 * vh / vw).round() as u32).max(1))
    } else {
        (((32.0 * vw / vh).round() as u32).max(1), 32)
    };
    if glow.texture == 0 {
        gles.GenTextures(1, &mut glow.texture);
        gles.GenFramebuffersOES(1, &mut glow.framebuffer);
    }
    gles.BindTexture(gles11::TEXTURE_2D, glow.texture);
    if glow.size != size {
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
        glow.size = size;
        glow.filled = false;
    }

    // The colour must reach the texture (the presenter normally replaces it).
    let mut old_env_mode: GLint = 0;
    gles.GetTexEnviv(gles11::TEXTURE_ENV, gles11::TEXTURE_ENV_MODE, &mut old_env_mode);
    gles.TexEnvi(gles11::TEXTURE_ENV, gles11::TEXTURE_ENV_MODE, gles11::MODULATE as _);
    gles.EnableClientState(gles11::VERTEX_ARRAY);
    gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
    let tex_coords: [f32; 12] = [0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    gles.TexCoordPointer(2, gles11::FLOAT, 0, tex_coords.as_ptr() as *const GLvoid);
    gles.Enable(gles11::TEXTURE_2D);

    // Blend the frame in: an exponential moving average over about
    // `smoothing` seconds (at 60 fps), the first frame filling it outright.
    gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, glow.framebuffer);
    gles.FramebufferTexture2DOES(
        gles11::FRAMEBUFFER_OES,
        gles11::COLOR_ATTACHMENT0_OES,
        gles11::TEXTURE_2D,
        glow.texture,
        0,
    );
    gles.Viewport(0, 0, size.0 as _, size.1 as _);
    gles.BindTexture(gles11::TEXTURE_2D, frame_texture);
    let matrix = centred_rotation(rotation_matrix);
    gles.MatrixMode(gles11::TEXTURE);
    gles.LoadMatrixf(matrix.columns().as_ptr() as *const _);
    gles.Enable(gles11::BLEND);
    gles.BlendFunc(gles11::SRC_ALPHA, gles11::ONE_MINUS_SRC_ALPHA);
    let weight = if glow.filled {
        1.0 - (-1.0 / (60.0 * smoothing.max(0.001))).exp()
    } else {
        1.0
    };
    // Four samples per glow pixel, a quarter of a pixel apart.
    let (qx, qy) = (0.5 / size.0 as f32, 0.5 / size.1 as f32);
    for (i, (dx, dy)) in [(-qx, -qy), (qx, -qy), (-qx, qy), (qx, qy)].into_iter().enumerate() {
        let mut quad = *vertices;
        for v in quad.chunks_exact_mut(2) {
            v[0] += dx;
            v[1] += dy;
        }
        // Equal shares of `weight` for the four samples.
        let alpha = if glow.filled { weight / 4.0 } else { 1.0 / (i + 1) as f32 };
        gles.Color4f(1.0, 1.0, 1.0, alpha);
        gles.VertexPointer(2, gles11::FLOAT, 0, quad.as_ptr() as *const GLvoid);
        gles.DrawArrays(gles11::TRIANGLES, 0, 6);
    }
    glow.filled = true;
    gles.Disable(gles11::BLEND);
    gles.LoadIdentity();
    gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, 0);

    // Stretch it across the screen: bright and saturated beside the picture,
    // fading to the dimmed glow at the edges.
    gles.Viewport(0, 0, screen_size.0 as _, screen_size.1 as _);
    gles.BindTexture(gles11::TEXTURE_2D, glow.texture);
    if glow.program == 0 {
        build_glow_program(gles, glow);
    }
    if glow.program != u32::MAX {
        let (sw, sh) = (screen_size.0.max(1) as f32, screen_size.1.max(1) as f32);
        let (x0, y0) = (viewport.0 as f32 / sw, viewport.1 as f32 / sh);
        let (x1, y1) = (x0 + vw / sw, y0 + vh / sh);
        // Fade across the full width of the bars (in screen heights).
        let fade = ((sw - vw) / 2.0 / sh).max((sh - vh) / 2.0 / sh).max(0.02);
        gles.UseProgram(glow.program);
        gles.Uniform1iv(glow.uniforms[0], 1, [0].as_ptr());
        gles.Uniform4fv(glow.uniforms[1], 1, [x0, y0, x1, y1].as_ptr());
        gles.Uniform4fv(
            glow.uniforms[2],
            1,
            [near_brightness, brightness, near_saturation, fade].as_ptr(),
        );
        gles.Uniform4fv(glow.uniforms[3], 1, [sw / sh, 0.0, 0.0, 0.0].as_ptr());
        gles.VertexAttribPointer(0, 2, gles11::FLOAT, gles11::FALSE, 0, vertices.as_ptr() as *const GLvoid);
        gles.EnableVertexAttribArray(0);
        gles.DrawArrays(gles11::TRIANGLES, 0, 6);
        gles.DisableVertexAttribArray(0);
        gles.UseProgram(0);
    } else {
        gles.Color4f(brightness, brightness, brightness, 1.0);
        gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
        gles.EnableClientState(gles11::VERTEX_ARRAY);
        gles.DrawArrays(gles11::TRIANGLES, 0, 6);
    }

    gles.Color4f(1.0, 1.0, 1.0, 1.0);
    gles.TexEnvi(gles11::TEXTURE_ENV, gles11::TEXTURE_ENV_MODE, old_env_mode);
}

/// Private fork, `--edge-bars`: fill the bars beside an upright picture with
/// solid colours, each easing towards its target (the average of that edge of
/// the picture, see `sample_edge_bar_colours`) over about `SMOOTHING` seconds.
/// With a pattern (`--edge-bars-texture`), the colour is divided by the
/// pattern's average, to undo the averaging, and the pattern is tiled over
/// the bars multiplied by it, starting at the picture's edges. The viewport
/// is left as the picture's.
unsafe fn draw_edge_bars(
    gles: &mut dyn GLES,
    edge_bars: EdgeBarsArgs<'_>,
    viewport: (u32, u32, u32, u32),
    vertices: &[f32; 12],
) {
    use gles11::types::*;
    const SMOOTHING: f32 = 0.4;

    let EdgeBarsArgs { state, target, pattern } = edge_bars;
    let weight = if state.filled { 1.0 - (-1.0 / (60.0 * SMOOTHING)).exp() } else { 1.0 };
    for (colour, target) in state.colours.iter_mut().zip(target) {
        for (c, t) in colour.iter_mut().zip(target) {
            *c += (t - *c) * weight;
        }
    }
    state.filled = true;

    let (vx, vy, vw, vh) = (viewport.0 as i32, viewport.1 as i32, viewport.2 as i32, viewport.3 as i32);
    let mut old_texture: GLint = 0;
    gles.GetIntegerv(gles11::TEXTURE_BINDING_2D, &mut old_texture);
    let mut old_env_mode: GLint = 0;
    gles.GetTexEnviv(gles11::TEXTURE_ENV, gles11::TEXTURE_ENV_MODE, &mut old_env_mode);
    gles.EnableClientState(gles11::VERTEX_ARRAY);
    gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
    if let Some(pattern) = pattern {
        gles.BindTexture(gles11::TEXTURE_2D, pattern.texture);
        gles.TexEnvi(gles11::TEXTURE_ENV, gles11::TEXTURE_ENV_MODE, gles11::MODULATE as _);
        gles.Enable(gles11::TEXTURE_2D);
        gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
    } else {
        gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
        gles.Disable(gles11::TEXTURE_2D);
    }
    // The picture is centred, so each bar is as wide as its left margin.
    let bars = [(0, state.colours[0]), (vx + vw, state.colours[1])];
    for (side, (x, colour)) in bars.into_iter().enumerate() {
        let [r, g, b] = match pattern {
            Some(pattern) => [0, 1, 2].map(|i| (colour[i] / pattern.average[i]).min(1.0)),
            None => colour,
        };
        if let Some(EdgePattern { size: (tw, th), texel, scroll, .. }) = pattern {
            // The pattern repeats across the bar from the picture's edge, and
            // moves up as the picture's edges scroll up.
            let across = vx as f32 / (tw as f32 * texel);
            let down = vh as f32 / (th as f32 * texel);
            let v0 = -scroll / th as f32;
            let (u0, u1) = if side == 0 { (-across, 0.0) } else { (0.0, across) };
            let (v1, v2) = (v0, v0 + down);
            let tex_coords: [f32; 12] = [u0, v1, u0, v2, u1, v1, u1, v1, u0, v2, u1, v2];
            gles.TexCoordPointer(2, gles11::FLOAT, 0, tex_coords.as_ptr() as *const GLvoid);
        }
        gles.Color4f(r, g, b, 1.0);
        gles.Viewport(x, vy, vx, vh);
        gles.DrawArrays(gles11::TRIANGLES, 0, 6);
    }
    gles.Color4f(1.0, 1.0, 1.0, 1.0);
    gles.TexEnvi(gles11::TEXTURE_ENV, gles11::TEXTURE_ENV_MODE, old_env_mode);
    gles.BindTexture(gles11::TEXTURE_2D, old_texture as GLuint);
    gles.Enable(gles11::TEXTURE_2D);
    gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
    gles.Viewport(vx, vy, vw, vh);
}

/// Private fork, `--starfield-bars`: move the stars on and draw them, added
/// onto the black bars, each bar in its own viewport so none spill onto the
/// picture. A star that falls off the bottom comes back at the top as a new
/// one. The viewport is left as the picture's.
unsafe fn draw_starfield(
    gles: &mut dyn GLES,
    starfield: StarfieldArgs<'_>,
    viewport: (u32, u32, u32, u32),
    vertices: &[f32; 12],
) {
    use gles11::types::*;
    const DOT_SIZE: usize = 32;

    let StarfieldArgs { state, points_high, warp } = starfield;
    let (vx, vy, vw, vh) = (viewport.0 as i32, viewport.1 as i32, viewport.2 as i32, viewport.3 as i32);
    if vx <= 0 || vh <= 0 {
        return;
    }
    let high = points_high.max(1) as f32;
    let points_per_pixel = high / vh as f32;
    let bar_wide = vx as f32 * points_per_pixel;

    // Keep the density as the bars change size (e.g. leaving the window).
    let now = std::time::Instant::now();
    let dt = state.last_frame.map_or(0.0, |last| (now - last).as_secs_f32().min(0.1));
    state.last_frame = Some(now);
    // The dots to draw: (bar, centre x within the bar, centre y, half width,
    // half height, colour).
    let mut dots: Vec<(usize, f32, f32, f32, f32, [f32; 3])> = Vec::new();
    if warp {
        let half_picture = vw as f32 * points_per_pixel / 2.0;
        let (reach_x, reach_y) = (half_picture + bar_wide + 20.0, high / 2.0 + 20.0);
        while state.warp_stars.len() < WARP_STARS {
            let star = state.new_warp_star(true);
            state.warp_stars.push(star);
        }
        for i in 0..state.warp_stars.len() {
            let star = &mut state.warp_stars[i];
            star.z -= dt;
            if star.z <= 0.05
                || (star.x / star.z).abs() > reach_x
                || (star.y / star.z).abs() > reach_y
            {
                state.warp_stars[i] = state.new_warp_star(false);
                continue;
            }
            let (sx, sy) = (star.x / star.z, star.y / star.z);
            // Drawn sizes are measured ones / 0.8: the measurements counted
            // pixels above 60 of 255, out to 0.8 of the dot's radius.
            let half = (30.0 / star.z).clamp(4.5, 17.0) / 2.0;
            let brightness = (1.0 - (star.z - 4.0) * 0.055).clamp(0.6, 1.0);
            let (side, x) = if sx < 0.0 {
                (0, sx + half_picture + bar_wide)
            } else {
                (1, sx - half_picture)
            };
            if (-half..bar_wide + half).contains(&x) {
                dots.push((side, x, high / 2.0 + sy, half, half, [brightness; 3]));
            }
        }
    } else {
        let count = (STAR_DENSITY * 2.0 * bar_wide * high).round() as usize;
        state.stars.retain(|star| star.x < 2.0 * bar_wide);
        state.stars.truncate(count);
        while state.stars.len() < count {
            let y = state.random() * high;
            let star = state.new_star(2.0 * bar_wide, y);
            state.stars.push(star);
        }
        for i in 0..state.stars.len() {
            let star = &mut state.stars[i];
            star.y += star.speed * dt;
            if star.y - star.size * STAR_TALLNESS / 2.0 > high {
                let mut star = state.new_star(2.0 * bar_wide, 0.0);
                star.y = -star.size * STAR_TALLNESS / 2.0;
                state.stars[i] = star;
            }
        }
        for star in &state.stars {
            let side = (star.x >= bar_wide) as usize;
            let (hw, hh) = (star.size / 2.0, star.size * STAR_TALLNESS / 2.0);
            dots.push((side, star.x - side as f32 * bar_wide, star.y, hw, hh, star.colour));
        }
    }

    let mut old_texture: GLint = 0;
    gles.GetIntegerv(gles11::TEXTURE_BINDING_2D, &mut old_texture);
    let mut old_env_mode: GLint = 0;
    gles.GetTexEnviv(gles11::TEXTURE_ENV, gles11::TEXTURE_ENV_MODE, &mut old_env_mode);
    if state.texture == 0 {
        // Like the game's star sprite: solid in the middle, fading evenly to
        // nothing at the edge.
        let mut pixels = Vec::with_capacity(DOT_SIZE * DOT_SIZE * 4);
        for y in 0..DOT_SIZE {
            for x in 0..DOT_SIZE {
                let d = |i: usize| (i as f32 + 0.5) / DOT_SIZE as f32 * 2.0 - 1.0;
                let r = (d(x).powi(2) + d(y).powi(2)).sqrt();
                let v = (((1.0 - r) / 0.85).clamp(0.0, 1.0) * 255.0) as u8;
                pixels.extend_from_slice(&[v, v, v, v]);
            }
        }
        gles.GenTextures(1, &mut state.texture);
        gles.BindTexture(gles11::TEXTURE_2D, state.texture);
        gles.TexImage2D(
            gles11::TEXTURE_2D,
            0,
            gles11::RGBA as _,
            DOT_SIZE as _,
            DOT_SIZE as _,
            0,
            gles11::RGBA,
            gles11::UNSIGNED_BYTE,
            pixels.as_ptr() as *const _,
        );
        for param in [gles11::TEXTURE_MIN_FILTER, gles11::TEXTURE_MAG_FILTER] {
            gles.TexParameteri(gles11::TEXTURE_2D, param, gles11::LINEAR as _);
        }
        for param in [gles11::TEXTURE_WRAP_S, gles11::TEXTURE_WRAP_T] {
            gles.TexParameteri(gles11::TEXTURE_2D, param, gles11::CLAMP_TO_EDGE as _);
        }
    }
    gles.BindTexture(gles11::TEXTURE_2D, state.texture);
    gles.TexEnvi(gles11::TEXTURE_ENV, gles11::TEXTURE_ENV_MODE, gles11::MODULATE as _);
    gles.Enable(gles11::TEXTURE_2D);
    gles.Enable(gles11::BLEND);
    gles.BlendFunc(gles11::ONE, gles11::ONE);
    gles.EnableClientState(gles11::VERTEX_ARRAY);
    gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
    let tex_coords: [f32; 12] = [0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    gles.TexCoordPointer(2, gles11::FLOAT, 0, tex_coords.as_ptr() as *const GLvoid);
    for (side, x) in [0, vx + vw].into_iter().enumerate() {
        gles.Viewport(x, vy, vx, vh);
        for &(_, cx, cy, hw, hh, colour) in dots.iter().filter(|d| d.0 == side) {
            let mut quad = *vertices;
            for i in (0..quad.len()).step_by(2) {
                quad[i] = (cx + quad[i] * hw) / bar_wide * 2.0 - 1.0;
                quad[i + 1] = 1.0 - (cy - quad[i + 1] * hh) / high * 2.0;
            }
            let [r, g, b] = colour;
            gles.Color4f(r, g, b, 1.0);
            gles.VertexPointer(2, gles11::FLOAT, 0, quad.as_ptr() as *const GLvoid);
            gles.DrawArrays(gles11::TRIANGLES, 0, 6);
        }
    }
    gles.Disable(gles11::BLEND);
    gles.Color4f(1.0, 1.0, 1.0, 1.0);
    gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
    gles.TexEnvi(gles11::TEXTURE_ENV, gles11::TEXTURE_ENV_MODE, old_env_mode);
    gles.BindTexture(gles11::TEXTURE_2D, old_texture as GLuint);
    gles.Viewport(vx, vy, vw, vh);
}

/// Private fork, `--edge-bars-texture`: upload the pattern to a new texture
/// in the current context: repeating, smoothly filtered (as games draw such
/// textures). The texture binding is left changed.
pub unsafe fn upload_pattern(gles: &mut dyn GLES, image: &crate::image::Image) -> u32 {
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
    for param in [gles11::TEXTURE_WRAP_S, gles11::TEXTURE_WRAP_T] {
        gles.TexParameteri(gles11::TEXTURE_2D, param, gles11::REPEAT as _);
    }
    texture
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
