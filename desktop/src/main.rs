#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod desktop_app;
mod gateway;

fn main() {
    desktop_app::run();
}
