/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Parsing and management of user-configurable options, e.g. for input methods.

use crate::gles::GLESImplementation;
use crate::window::{DeviceFamily, DeviceOrientation};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::net::{SocketAddr, ToSocketAddrs};
use std::num::NonZeroU32;
use std::path::PathBuf;

pub const OPTIONS_HELP: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/OPTIONS_HELP.txt"));

/// Game controller button for `--button-to-touch=` option.
#[derive(Copy, Clone, Hash, PartialEq, Eq, Debug)]
pub enum Button {
    DPadLeft,
    DPadUp,
    DPadRight,
    DPadDown,
    Start,
    A,
    B,
    X,
    Y,
    LeftShoulder,
    /// Private fork.
    Back,
}

fn parse_button(name: &str, option: &str) -> Result<Button, String> {
    match name {
        "DPadLeft" => Ok(Button::DPadLeft),
        "DPadUp" => Ok(Button::DPadUp),
        "DPadRight" => Ok(Button::DPadRight),
        "DPadDown" => Ok(Button::DPadDown),
        "Start" => Ok(Button::Start),
        "A" => Ok(Button::A),
        "B" => Ok(Button::B),
        "X" => Ok(Button::X),
        "Y" => Ok(Button::Y),
        "LeftShoulder" => Ok(Button::LeftShoulder),
        "Back" => Ok(Button::Back),
        _ => Err(format!("Invalid button for {option}")),
    }
}

/// Parses `BUTTON,X,Y` as used by `--button-to-touch=`.
fn parse_button_to_touch(values: &str, option: &str) -> Result<(Button, (f32, f32)), String> {
    let (button, coords) = values
        .split_once(',')
        .ok_or_else(|| format!("{option} requires three values"))?;
    let (x, y) = coords
        .split_once(',')
        .ok_or_else(|| format!("{option} requires three values"))?;
    let button = parse_button(button, option)?;
    let x: f32 = x
        .parse()
        .map_err(|_| format!("Invalid X co-ordinate for {option}"))?;
    let y: f32 = y
        .parse()
        .map_err(|_| format!("Invalid Y co-ordinate for {option}"))?;
    Ok((button, (x, y)))
}

/// Struct containing all user-configurable options.
#[derive(Clone)]
pub struct Options {
    pub fullscreen: bool,
    /// Private fork: scale the app's frame to the window without filtering.
    pub nearest_present: bool,
    /// Private fork: force nearest-neighbour filtering of app textures.
    pub nearest_textures: bool,
    pub device_family: Option<DeviceFamily>,
    pub initial_orientation: DeviceOrientation,
    pub scale_hack: NonZeroU32,
    /// Private fork: see `--window-scale=` in OPTIONS_HELP.txt.
    pub window_scale: NonZeroU32,
    /// Private fork: see `--dpi-aware` in OPTIONS_HELP.txt.
    pub dpi_aware: bool,
    /// Private fork: see `--integer-scaling` in OPTIONS_HELP.txt.
    pub integer_scaling: bool,
    /// Private fork: ignore --integer-scaling in landscape and fill the screen.
    pub landscape_fill_screen: bool,
    /// Private fork: picture drawn behind the app in fullscreen portrait.
    pub bezel: Option<String>,
    /// Private fork: regions (x, y, width, height, in portrait screen points)
    /// moved from the app's picture into the bezel's right-hand margin.
    pub move_to_margin: Vec<(f32, f32, f32, f32)>,
    /// Private fork: size of the moved regions relative to the app's picture.
    pub margin_scale: f32,
    pub deadzone: f32,
    pub analog_stick_tilt_controls: bool,
    pub x_tilt_range: f32,
    pub y_tilt_range: f32,
    pub x_tilt_offset: f32,
    pub y_tilt_offset: f32,
    pub button_to_touch: HashMap<Button, (f32, f32)>,
    /// Private fork: replaces `button_to_touch` while the device is landscape.
    pub landscape_button_to_touch: HashMap<Button, (f32, f32)>,
    /// Private fork: this button turns the device between portrait and
    /// landscape.
    pub button_to_rotate: Option<Button>,
    pub dpad_to_touch: Option<(f32, f32, f32, f32)>,
    pub stick_to_touch: Option<(f32, f32, f32, f32)>,
    /// See [crate::window::Window::update_stick_swipe] (private fork).
    pub stick_to_swipe: Option<f32>,
    /// Private fork: mouse movement drives the swipe finger, with this
    /// sensitivity. Also hides the cursor over the app.
    pub mouse_to_swipe: Option<f32>,
    /// Private fork: where the swipe finger touches down (screen points).
    pub swipe_origin: Option<(f32, f32)>,
    /// Private fork: this button switches the swipe finger to a virtual stick
    /// of this radius (screen points) at the swipe origin, and back.
    pub stick_mode_toggle: Option<(Button, f32)>,
    /// Private fork: the left stick types iCade joystick letters.
    pub stick_to_icade: bool,
    /// Private fork: see [crate::gridrunner]. Acceleration per game tick and
    /// top speed, both relative to the game's own full speed.
    pub icade_analog: Option<(f32, f32)>,
    pub stabilize_virtual_cursor: Option<(f32, f32)>,
    pub gles1_implementation: Option<GLESImplementation>,
    pub direct_memory_access: bool,
    pub gdb_listen_addrs: Option<Vec<SocketAddr>>,
    pub preferred_languages: Option<Vec<String>>,
    pub headless: bool,
    pub print_fps: bool,
    pub fps_limit: Option<f64>,
    pub force_composition: bool,
    pub network_access: bool,
    pub popup_errors: bool,
    pub dumping_options: DumpingOptions,
    pub dumping_file: PathBuf,
    pub ignore_gl_errors: bool,
    pub zero_stack_after_guest_to_host_call: Option<u32>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            fullscreen: false,
            nearest_present: false,
            window_scale: NonZeroU32::new(1).unwrap(),
            dpi_aware: false,
            integer_scaling: false,
            landscape_fill_screen: false,
            bezel: None,
            move_to_margin: Vec::new(),
            margin_scale: 1.5,
            nearest_textures: false,
            device_family: None,
            initial_orientation: DeviceOrientation::Portrait,
            scale_hack: NonZeroU32::new(1).unwrap(),
            analog_stick_tilt_controls: true,
            deadzone: 0.1,
            x_tilt_range: 60.0,
            y_tilt_range: 60.0,
            x_tilt_offset: 0.0,
            y_tilt_offset: 0.0,
            button_to_touch: HashMap::new(),
            landscape_button_to_touch: HashMap::new(),
            button_to_rotate: None,
            dpad_to_touch: None,
            stick_to_touch: None,
            stick_to_swipe: None,
            mouse_to_swipe: None,
            swipe_origin: None,
            stick_mode_toggle: None,
            stick_to_icade: false,
            icade_analog: None,
            stabilize_virtual_cursor: None,
            gles1_implementation: None,
            direct_memory_access: true,
            gdb_listen_addrs: None,
            preferred_languages: None,
            headless: false,
            print_fps: false,
            fps_limit: Some(60.0), // Original iPhone is 60Hz and uses v-sync,
            force_composition: false,
            network_access: false,
            popup_errors: true,
            dumping_options: Default::default(),
            dumping_file: crate::paths::user_data_base_path().join("DUMP.txt"),
            ignore_gl_errors: false,
            zero_stack_after_guest_to_host_call: None,
        }
    }
}

impl Options {
    /// Parse the command-line argument syntax for an option. Returns `Ok(true)`
    /// if the option was valid and has been applied, or `Ok(false)` if the
    /// option was not recognized.
    pub fn parse_argument(&mut self, arg: &str) -> Result<bool, String> {
        fn parse_degrees(arg: &str, name: &str) -> Result<f32, String> {
            let arg: f32 = arg
                .parse()
                .map_err(|_| format!("Value for {name} is invalid"))?;
            if !arg.is_finite() || !(-360.0..=360.0).contains(&arg) {
                return Err(format!("Value for {name} is out of range"));
            }
            Ok(arg)
        }

        if arg == "--fullscreen" {
            self.fullscreen = true;
        } else if let Some(value) = arg.strip_prefix("--window-scale=") {
            self.window_scale = value
                .parse()
                .map_err(|_| "Invalid window scale factor".to_string())?;
        } else if arg == "--dpi-aware" {
            self.dpi_aware = true;
        } else if arg == "--integer-scaling" {
            self.integer_scaling = true;
        } else if arg == "--landscape-fill-screen" {
            self.landscape_fill_screen = true;
        } else if let Some(path) = arg.strip_prefix("--bezel=") {
            self.bezel = Some(path.to_string());
        } else if let Some(values) = arg.strip_prefix("--move-to-margin=") {
            let nums: [f32; 4] = values
                .split(',')
                .map(|s| s.parse::<f32>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "invalid --move-to-margin".to_string())?
                .try_into()
                .map_err(|_| "--move-to-margin= requires four values".to_string())?;
            self.move_to_margin
                .push((nums[0], nums[1], nums[2], nums[3]));
        } else if let Some(value) = arg.strip_prefix("--margin-scale=") {
            self.margin_scale = value
                .parse()
                .ok()
                .filter(|&s: &f32| s > 0.0)
                .ok_or_else(|| "--margin-scale= requires a positive number".to_string())?;
        } else if arg == "--nearest-present" {
            self.nearest_present = true;
        } else if arg == "--nearest-textures" {
            self.nearest_textures = true;
        } else if arg == "--upside-down" {
            self.initial_orientation = DeviceOrientation::PortraitUpsideDown;
        } else if arg == "--landscape-left" {
            self.initial_orientation = DeviceOrientation::LandscapeLeft;
        } else if arg == "--landscape-right" {
            self.initial_orientation = DeviceOrientation::LandscapeRight;
        } else if let Some(value) = arg.strip_prefix("--device-family=") {
            let parsed =
                DeviceFamily::try_from(value).map_err(|_| "Invalid device family".to_string())?;
            self.device_family = Some(parsed);
        } else if let Some(value) = arg.strip_prefix("--scale-hack=") {
            self.scale_hack = value
                .parse()
                .map_err(|_| "Invalid scale hack factor".to_string())?;
        } else if arg == "--disable-analog-stick-tilt-controls" {
            self.analog_stick_tilt_controls = false;
        } else if let Some(value) = arg.strip_prefix("--deadzone=") {
            self.deadzone = parse_degrees(value, "deadzone")?;
        } else if let Some(value) = arg.strip_prefix("--x-tilt-range=") {
            self.x_tilt_range = parse_degrees(value, "X tilt range")?;
        } else if let Some(value) = arg.strip_prefix("--y-tilt-range=") {
            self.y_tilt_range = parse_degrees(value, "Y tilt range")?;
        } else if let Some(value) = arg.strip_prefix("--x-tilt-offset=") {
            self.x_tilt_offset = parse_degrees(value, "X tilt offset")?;
        } else if let Some(value) = arg.strip_prefix("--y-tilt-offset=") {
            self.y_tilt_offset = parse_degrees(value, "Y tilt offset")?;
        } else if let Some(values) = arg.strip_prefix("--button-to-touch=") {
            let (button, coords) = parse_button_to_touch(values, "--button-to-touch=")?;
            self.button_to_touch.insert(button, coords);
        } else if let Some(values) = arg.strip_prefix("--landscape-button-to-touch=") {
            let (button, coords) =
                parse_button_to_touch(values, "--landscape-button-to-touch=")?;
            self.landscape_button_to_touch.insert(button, coords);
        } else if let Some(value) = arg.strip_prefix("--button-to-rotate=") {
            self.button_to_rotate = Some(parse_button(value, "--button-to-rotate=")?);
        } else if let Some(values) = arg.strip_prefix("--stick-to-touch=") {
            let nums: [f32; 4] = values
                .split(',')
                .map(|s| s.parse::<f32>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "invalid --stick-to-touch".to_string())?
                .try_into()
                .map_err(|_| "--stick-to-touch= requires four values".to_string())?;

            self.stick_to_touch = Some((nums[0], nums[1], nums[2], nums[3]));
        } else if let Some(value) = arg.strip_prefix("--mouse-to-swipe=") {
            let sensitivity: f32 = value
                .parse()
                .ok()
                .filter(|&s: &f32| s > 0.0)
                .ok_or_else(|| "--mouse-to-swipe= requires a positive sensitivity".to_string())?;
            self.mouse_to_swipe = Some(sensitivity);
        } else if let Some(value) = arg.strip_prefix("--swipe-origin=") {
            let (x, y) = value
                .split_once(',')
                .ok_or_else(|| "--swipe-origin= requires two values".to_string())?;
            let x: f32 = x.parse().map_err(|_| "Invalid --swipe-origin= X".to_string())?;
            let y: f32 = y.parse().map_err(|_| "Invalid --swipe-origin= Y".to_string())?;
            self.swipe_origin = Some((x, y));
        } else if let Some(value) = arg.strip_prefix("--stick-to-swipe=") {
            let speed: f32 = value
                .parse()
                .ok()
                .filter(|&s: &f32| s > 0.0)
                .ok_or_else(|| "--stick-to-swipe= requires a positive speed".to_string())?;
            self.stick_to_swipe = Some(speed);
        } else if arg == "--stick-to-icade" {
            self.stick_to_icade = true;
        } else if let Some(value) = arg.strip_prefix("--icade-analog=") {
            let (accel, top_speed) = value.split_once(',').unwrap_or((value, "1"));
            let accel: f32 = accel
                .parse()
                .ok()
                .filter(|&a: &f32| a > 0.0 && a <= 2.0)
                .ok_or_else(|| "--icade-analog= requires an acceleration in (0, 2]".to_string())?;
            let top_speed: f32 = top_speed
                .parse()
                .ok()
                .filter(|&s: &f32| s > 0.0 && s <= 4.0)
                .ok_or_else(|| "--icade-analog= top speed must be in (0, 4]".to_string())?;
            self.icade_analog = Some((accel, top_speed));
        } else if let Some(value) = arg.strip_prefix("--stick-mode-toggle=") {
            let (button, radius) = value
                .split_once(',')
                .ok_or_else(|| "--stick-mode-toggle= requires two values".to_string())?;
            let button = parse_button(button, "--stick-mode-toggle=")?;
            let radius: f32 = radius
                .parse()
                .ok()
                .filter(|&r: &f32| r > 0.0)
                .ok_or_else(|| "--stick-mode-toggle= requires a positive radius".to_string())?;
            self.stick_mode_toggle = Some((button, radius));
        } else if let Some(values) = arg.strip_prefix("--dpad-to-touch=") {
            let nums: [f32; 4] = values
                .split(',')
                .map(|s| s.parse::<f32>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "invalid --dpad-to-touch".to_string())?
                .try_into()
                .map_err(|_| "--dpad-to-touch= requires four values".to_string())?;

            self.dpad_to_touch = Some((nums[0], nums[1], nums[2], nums[3]));
        } else if let Some(value) = arg.strip_prefix("--stabilize-virtual-cursor=") {
            let (smoothing_strength, sticky_radius) = value
                .split_once(',')
                .ok_or_else(|| "--stabilize-virtual-cursor= requires two values".to_string())?;
            let smoothing_strength: f32 = smoothing_strength
                .parse()
                .ok()
                .filter(|&s| s >= 0.0)
                .ok_or_else(|| {
                    "Invalid smoothing strength for --stabilize-virtual-cursor=".to_string()
                })?;
            let sticky_radius: f32 = sticky_radius
                .parse()
                .ok()
                .filter(|&s| s >= 0.0)
                .ok_or_else(|| {
                    "Invalid sticky radius for --stabilize-virtual-cursor=".to_string()
                })?;
            self.stabilize_virtual_cursor = Some((smoothing_strength, sticky_radius));
        } else if let Some(value) = arg.strip_prefix("--gles1=") {
            self.gles1_implementation = Some(
                GLESImplementation::from_short_name(value)
                    .map_err(|_| "Unrecognized --gles1= value".to_string())?,
            );
        } else if arg == "--disable-direct-memory-access" {
            self.direct_memory_access = false;
        } else if let Some(address) = arg.strip_prefix("--gdb=") {
            let addrs = address
                .to_socket_addrs()
                .map_err(|e| format!("Could not resolve GDB server listen address: {e}"))?
                .collect();
            self.gdb_listen_addrs = Some(addrs);
        } else if let Some(value) = arg.strip_prefix("--preferred-languages=") {
            self.preferred_languages = Some(value.split(',').map(ToOwned::to_owned).collect());
        } else if arg == "--headless" {
            self.headless = true;
            // Can't show the dialog box when headless!
            self.popup_errors = false;
        } else if arg == "--print-fps" {
            self.print_fps = true;
        } else if let Some(value) = arg.strip_prefix("--fps-limit=") {
            if value == "off" {
                self.fps_limit = None;
            } else {
                let limit: f64 = value
                    .parse()
                    .ok()
                    .filter(|&v| v > 0.0)
                    .ok_or_else(|| "Invalid value for --fps-limit=".to_string())?;
                self.fps_limit = Some(limit);
            }
        } else if arg == "--force-composition" {
            self.force_composition = true;
        } else if arg == "--allow-network-access" {
            self.network_access = true;
        } else if arg == "--no-error-popup" {
            self.popup_errors = false;
        } else if let Some(values) = arg.strip_prefix("--dump=") {
            self.dumping_options = parse_dump_options(values)?;
        } else if let Some(path) = arg.strip_prefix("--dump-file=") {
            self.dumping_file = crate::paths::user_data_base_path().join(path);
        } else if arg == "--ignore-gl-errors" {
            self.ignore_gl_errors = true;
        } else if let Some(value) = arg.strip_prefix("--zero-stack-after-guest-to-host-call=") {
            self.zero_stack_after_guest_to_host_call = Some(value.parse().map_err(|_| {
                "Invalid value for --zero-stack-after-guest-to-host-call=".to_string()
            })?);
        } else {
            return Ok(false);
        };
        Ok(true)
    }
}

/// Try to get app-specific options from a file.
///
/// Returns [Ok] if there is no error when reading the file, otherwise [Err].
/// The [Ok] value is a [Some] with the options if they could be found, or
/// [None] if no options were found for this app.
pub fn get_options_from_file<F: Read>(file: F, app_id: &str) -> Result<Option<String>, String> {
    let file = BufReader::new(file);
    for (line_no, line) in BufRead::lines(file).enumerate() {
        // Line numbering usually starts from 1
        let line_no = line_no + 1;

        let line = line.map_err(|e| format!("Error while reading line {line_no}: {e}"))?;

        // # for single-line comments
        let line = if let Some((rest, _)) = line.split_once('#') {
            rest
        } else {
            &line
        };

        // Empty/all-comment lines ignored
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let (line_app_id, line_options) = line.split_once(':').ok_or_else(|| format!("Line {line_no} is not a comment and is missing a colon (:) to separate the app ID from the options"))?;
        let line_app_id = line_app_id.trim();

        if line_app_id != app_id {
            continue;
        }

        let line_options = line_options.trim();
        if line_options.is_empty() {
            return Ok(None);
        } else {
            return Ok(Some(line_options.to_string()));
        }
    }
    Ok(None)
}

#[derive(Default, Clone)]
pub struct DumpingOptions {
    pub linking_info: bool,
    pub symbols: bool,
}

impl DumpingOptions {
    /// Check if any of the dumping options are active.
    pub fn any(&self) -> bool {
        self.linking_info || self.symbols
    }
}

fn parse_dump_options(options: &str) -> Result<DumpingOptions, String> {
    let mut dumping_options = DumpingOptions::default();
    for opt in options.split(",") {
        if opt == "linking-info" {
            // Dumps linked symbols, classes and selectors for the given app
            dumping_options.linking_info = true;
        } else if opt == "symbols" {
            // Dumps touchHLE provided symbols and exits
            dumping_options.symbols = true;
        } else {
            return Err(format!("Unrecognized option {opt} for --dump=..."));
        }
    }
    Ok(dumping_options)
}
