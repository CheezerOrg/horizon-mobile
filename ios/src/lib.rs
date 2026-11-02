//! Horizon on iOS: the entry point `app/main.m` calls. It points the game at the app's sandbox,
//! then runs the same startup as the desktop executable; winit starts UIKit inside it.

use std::path::{Path, PathBuf};

mod account;
mod open;

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
    open::register();
    horizon_client::msa::restore();
    horizon_client::entry::main();
}

/// The app's Documents: the Files app shows it as On My iPhone › Horizon, so players add
/// resource packs, shader packs and worlds there (`UIFileSharingEnabled` in Info.plist).
fn home() -> PathBuf {
    let container = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    container.join("Documents")
}

/// The folders players put files in: made up front, so the Files app shows where they go.
const PLAYER_FOLDERS: [&str; 3] = ["resourcepacks", "shaderpacks", "saves"];

fn prepare_home(home: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    move_old_home(home);
    for folder in PLAYER_FOLDERS {
        std::fs::create_dir_all(home.join(folder))?;
    }
    // SAFETY: nothing else runs yet: this is the process's first thread, before the game starts any.
    unsafe {
        std::env::set_var("HORIZON_HOME", home);
        std::env::set_var("HORIZON_NO_DISCORD", "1");
    }
    // Saves, servers.dat and options.txt resolve from the working directory; the bundle is read-only.
    std::env::set_current_dir(home)?;
    write_default_options(home)
}

/// Earlier builds kept the game in `Library/Application Support/horizon`, which the Files app
/// can't show: its worlds, options and packs move to Documents (what is there already stays).
fn move_old_home(home: &Path) {
    let old = horizon_client::logging::data_dir().join("horizon");
    let Ok(entries) = std::fs::read_dir(&old) else {
        return;
    };
    for entry in entries.flatten() {
        let to = home.join(entry.file_name());
        if !to.exists()
            && let Err(e) = std::fs::rename(entry.path(), &to)
        {
            eprintln!(
                "Horizon: could not move {} to {}: {e}",
                entry.path().display(),
                to.display()
            );
        }
    }
    let _ = std::fs::remove_dir(&old);
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
