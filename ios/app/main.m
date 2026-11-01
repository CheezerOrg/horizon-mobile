// Horizon.app's entry: all of the app is Rust (ios/src/lib.rs); winit runs UIApplicationMain.
extern void horizon_main(void);

int main(int argc, char *argv[]) {
    (void)argc;
    (void)argv;
    horizon_main();
    return 0;
}
