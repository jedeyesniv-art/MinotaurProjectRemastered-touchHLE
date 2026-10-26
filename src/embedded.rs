/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Private fork: files built into the executable by the `embedded_game`
//! feature, so that a game is a single .exe (Gridrunner, and every other
//! touchHLE port in C:\Gridios).
//!
//! The files are read from the `embedded/` directory at build time (see
//! `build-single-exe.ps1` in the parent directory, which fills it): the game
//! as `game.ipa`, the options and options help written next to the exe if
//! missing, and everything else (touchHLE's libraries, a font, the default
//! options, files the options name such as a bezel) by its usual relative
//! path, listed by build.rs. Saves and the user's options still live next
//! to the executable.

use crate::fs::BundleData;

/// Stand-in app path meaning "the built-in game".
pub const BUILT_IN_APP: &str = "<built-in game>";

#[cfg(feature = "embedded_game")]
mod files {
    // RESOURCES: files otherwise found through [crate::paths::ResourceFile]
    // or next to the executable, by relative path.
    include!(concat!(env!("OUT_DIR"), "/embedded_files.rs"));

    macro_rules! embed {
        ($path:literal) => {
            (
                $path,
                include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/embedded/", $path))
                    as &[u8],
            )
        };
    }

    /// Files written next to the executable if they're missing, for the user
    /// to read and edit.
    pub const USER_FILES: &[(&str, &[u8])] =
        &[embed!("touchHLE_options.txt"), embed!("OPTIONS_HELP.txt")];

    pub const GAME_IPA: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/embedded/game.ipa"));
}

/// A built-in file by its usual relative path, if there is one. The games
/// draw their own text, so only one font is built in and it stands in for all
/// of them.
#[cfg(feature = "embedded_game")]
pub fn resource(path: &str) -> Option<&'static [u8]> {
    let path = path.replace('\\', "/");
    let find = |path: &str| {
        files::RESOURCES
            .iter()
            .find(|&&(name, _)| name == path)
            .map(|&(_, bytes)| bytes)
    };
    find(&path).or_else(|| {
        let is_font = path.starts_with(crate::paths::FONTS_DIR)
            && (path.ends_with(".ttf") || path.ends_with(".otf"));
        is_font
            .then(|| find("touchHLE_fonts/LiberationSans-Regular.ttf"))
            .flatten()
    })
}
#[cfg(not(feature = "embedded_game"))]
pub fn resource(_path: &str) -> Option<&'static [u8]> {
    None
}

/// Open the built-in game.
pub fn open_game() -> Result<BundleData, String> {
    #[cfg(feature = "embedded_game")]
    return BundleData::open_ipa_bytes(files::GAME_IPA);
    #[cfg(not(feature = "embedded_game"))]
    return Err("This build has no built-in app.".to_string());
}

/// Whether this build has a built-in game.
pub const fn has_game() -> bool {
    cfg!(feature = "embedded_game")
}

/// Write the user's options file and the options help next to the executable
/// if they aren't there yet.
pub fn write_missing_user_files() {
    #[cfg(feature = "embedded_game")]
    for &(name, bytes) in files::USER_FILES {
        let path = crate::paths::user_data_base_path().join(name);
        if !path.exists() {
            if let Err(e) = std::fs::write(&path, bytes) {
                echo!("Warning: Couldn't write {}: {}", path.display(), e);
            }
        }
    }
}
