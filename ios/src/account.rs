//! What the game's Microsoft sign-in (`horizon_client::msa`) needs from iOS: the refresh token in
//! the Keychain (this app's only, never backed up off the device in plain text) and Safari for the
//! page that asks for the code, which goes to the pasteboard first.
use block2::Block;
use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2_foundation::{NSDictionary, NSString, NSURL};
use security_framework::passwords;

const SERVICE: &str = "dev.playhorizon.game";
const ACCOUNT: &str = "microsoft-refresh-token";

pub fn register() {
    horizon_client::msa::set_platform(horizon_client::msa::Platform {
        load,
        save,
        delete,
        open,
    });
}

fn load() -> Option<String> {
    let bytes = passwords::get_generic_password(SERVICE, ACCOUNT).ok()?;
    String::from_utf8(bytes).ok().filter(|t| !t.is_empty())
}

fn save(token: &str) {
    if let Err(e) = passwords::set_generic_password(SERVICE, ACCOUNT, token.as_bytes()) {
        eprintln!("Horizon: could not keep the sign-in in the Keychain ({})", e.code());
    }
}

fn delete() {
    let _ = passwords::delete_generic_password(SERVICE, ACCOUNT);
}

/// Called from the event loop, on the main thread, as UIKit requires.
fn open(url: &str, code: &str) {
    let (Some(application), Some(pasteboard)) =
        (AnyClass::get(c"UIApplication"), AnyClass::get(c"UIPasteboard"))
    else {
        return;
    };
    // SAFETY: plain UIKit messages with their documented argument types, on the main thread.
    unsafe {
        let board: *mut AnyObject = msg_send![pasteboard, generalPasteboard];
        if !board.is_null() {
            let _: () = msg_send![board, setString: &*NSString::from_str(code)];
        }
        let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else {
            return;
        };
        let app: *mut AnyObject = msg_send![application, sharedApplication];
        if app.is_null() {
            return;
        }
        let options = NSDictionary::<NSString, AnyObject>::new();
        let done: Option<&Block<dyn Fn(Bool)>> = None;
        let _: () = msg_send![app, openURL: &*url, options: &*options, completionHandler: done];
    }
}
