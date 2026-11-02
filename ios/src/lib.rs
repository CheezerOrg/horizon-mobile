//! Horizon on iOS: the entry point `app/main.m` calls. It points the game at the app's sandbox,
//! then runs the same startup as the desktop executable; winit starts UIKit inside it.

use std::path::{Path, PathBuf};

mod account;

#[global_allocator]
static ALLOCATOR: horizon_threads::Allocator = horizon_threads::ALLOCATOR;

/// Never returns: UIKit owns the main thread until the app ends.
#[unsafe(no_mangle)]
pub extern "C" fn horizon_main() {
    let home = home();
    if let Err(e) = prepare_home(&home) {
        eprintln!("Horizon: could not prepare {}: {e}", home.display());
    }
    // The account signed in last time, renewed in the background while the game starts.
    account::register();
    horizon_client::msa::restore();
    horizon_client::entry::main();
}

/// `Library/Application Support/horizon` in the app's container (HOME is the container on iOS).
fn home() -> PathBuf {
    horizon_client::logging::data_dir().join("horizon")
}

fn prepare_home(home: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    // SAFETY: nothing else runs yet: this is the process's first thread, before the game starts any.
    unsafe {
        std::env::set_var("HORIZON_HOME", home);
        std::env::set_var("HORIZON_NO_DISCORD", "1");
    }
    // Saves, servers.dat and options.txt resolve from the working directory; the bundle is read-only.
    std::env::set_current_dir(home)?;
    write_default_options(home)
}

/// The first run's options for a phone or tablet: closer render and simulation distances than
/// the desktop's. Written once; the player's own changes are kept afterwards.
fn write_default_options(home: &Path) -> std::io::Result<()> {
    let options = home.join("options.txt");
    if options.exists() {
        return Ok(());
    }
    std::fs::write(options, "renderDistance:8\nsimulationDistance:6\n")
}
