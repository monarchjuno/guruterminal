#[cfg(windows)]
const WINDOWS_UI_THREAD_STACK_BYTES: usize = 8 * 1024 * 1024;

#[cfg(windows)]
fn main() {
    // Windows gives the process entry thread a much smaller stack than the
    // other supported desktop platforms. Keep all Tauri construction and the
    // UI event loop on a dedicated, explicitly sized thread so a debug build
    // with the full typed command surface cannot overflow during startup.
    std::thread::Builder::new()
        .name("guruterminal-ui".into())
        .stack_size(WINDOWS_UI_THREAD_STACK_BYTES)
        .spawn(guruterminal_desktop::run)
        .expect("failed to start the Guru Terminal UI thread")
        .join()
        .expect("Guru Terminal UI thread panicked");
}

#[cfg(not(windows))]
fn main() {
    guruterminal_desktop::run();
}
