#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    std::process::exit(idlebot::windows::app::main());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("discord-mobile-idle: the tray app only runs on Windows");
    std::process::exit(2);
}
