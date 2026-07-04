// Prevents console window from appearing on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    depo_audio_lib::run();
}
