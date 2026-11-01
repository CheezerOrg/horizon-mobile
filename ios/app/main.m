// Horizon.app's entry: all of the app is Rust (ios/src/lib.rs); winit runs UIApplicationMain.
#import <Foundation/Foundation.h>
#include <crt_externs.h>

extern void horizon_main(void);

int main(int argc, char *argv[]) {
    // The game's command line: the resources bundle_resources.sh copied into the app. They can't
    // be the `resources` folder the game looks for beside its executable: on the case-insensitive
    // file system, a top-level Resources folder makes iOS read the app as a macOS-style bundle.
    // Rust's std::env::args reads _NSGetArgv, so the game sees these arguments.
    static char *args[4];
    args[0] = argc > 0 ? argv[0] : "Horizon";
    args[1] = "--resources";
    args[2] = strdup([[NSBundle mainBundle].bundlePath stringByAppendingPathComponent:@"game"].fileSystemRepresentation);
    args[3] = NULL;
    *_NSGetArgv() = args;
    *_NSGetArgc() = 3;
    horizon_main();
    return 0;
}
