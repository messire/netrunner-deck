// No console window in any build: tracing goes to `<app log dir>/deck.log`
// (lib.rs `init_tracing`). A debug build started from a terminal (`npm run
// tauri dev`) attaches to that terminal so the log is still visible there —
// unless `DECK_DETACH=1`, which a launcher sets so Deck outlives its window.
#![windows_subsystem = "windows"]

fn main() {
    #[cfg(all(windows, debug_assertions))]
    if std::env::var_os("DECK_DETACH").is_none() {
        attach_parent_console();
    }
    netrunner_deck_lib::run()
}

#[cfg(all(windows, debug_assertions))]
fn attach_parent_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    // SAFETY: a plain Win32 call with a constant argument; it fails harmlessly
    // when the parent has no console (a double-click or Start-Process launch).
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}
