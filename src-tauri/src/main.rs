// Without this, a release build on Windows is a console-subsystem program and opens a
// terminal window behind the app every time it starts. Debug builds keep the console so
// `tauri dev` output stays visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    miesql_lib::run()
}
