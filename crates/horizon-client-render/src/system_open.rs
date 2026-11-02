//! ios-port: how the platform opens a folder or a link where spawning a desktop opener can't
//! (`open`, `xdg-open`, `explorer`): the iOS app opens folders in the Files app and links in Safari.
//! Without a hook set, callers fall back to their desktop opener.
use std::path::Path;
use std::sync::OnceLock;

/// What the platform does: open the folder, or the link.
pub struct Platform {
    pub open_folder: fn(&Path),
    pub open_uri: fn(&str),
}
static PLATFORM: OnceLock<Platform> = OnceLock::new();

pub fn set_platform(platform: Platform) {
    let _ = PLATFORM.set(platform);
}

/// Opens `dir` through the platform's hook; false when there is none.
pub fn open_folder(dir: &Path) -> bool {
    PLATFORM.get().map(|p| (p.open_folder)(dir)).is_some()
}

/// Opens `uri` through the platform's hook; false when there is none.
pub fn open_uri(uri: &str) -> bool {
    PLATFORM.get().map(|p| (p.open_uri)(uri)).is_some()
}
