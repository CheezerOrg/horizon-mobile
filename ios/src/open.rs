//! Folders and links the game opens (`horizon_client::system_open`): iOS has no `open` to spawn.
//! A folder opens in the Files app (On My iPhone › Horizon is the app's Documents, the game
//! folder), a link in Safari.
use std::path::Path;

use block2::Block;
use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2_foundation::{NSDictionary, NSString, NSURL};

pub fn register() {
    horizon_client::system_open::set_platform(horizon_client::system_open::Platform {
        open_folder,
        open_uri: open_url,
    });
}

/// The Files app's own scheme on the folder's path: `file:///…` becomes `shareddocuments:///…`,
/// which the Files app opens at that folder (it must be in this app's Documents).
fn open_folder(dir: &Path) {
    let Some(path) = dir.to_str() else {
        return;
    };
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    let Some(url) = url.absoluteString() else {
        return;
    };
    if let Some(rest) = url.to_string().strip_prefix("file://") {
        open_url(&format!("shareddocuments://{rest}"));
    }
}

/// Opens `url` with the app that handles it. Called from the event loop, on the main thread, as
/// UIKit requires.
pub fn open_url(url: &str) {
    let Some(application) = AnyClass::get(c"UIApplication") else {
        return;
    };
    let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return;
    };
    // SAFETY: plain UIKit messages with their documented argument types, on the main thread.
    unsafe {
        let app: *mut AnyObject = msg_send![application, sharedApplication];
        if app.is_null() {
            return;
        }
        let options = NSDictionary::<NSString, AnyObject>::new();
        let done: Option<&Block<dyn Fn(Bool)>> = None;
        let _: () = msg_send![app, openURL: &*url, options: &*options, completionHandler: done];
    }
}
