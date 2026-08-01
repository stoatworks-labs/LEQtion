// Suppress the console window on a Windows release build. Debug builds keep it,
// because that is where the tracing output goes.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    leqtion_lib::run()
}
