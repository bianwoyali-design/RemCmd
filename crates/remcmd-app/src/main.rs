#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

fn main() {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("RemCmd {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    remcmd_app::run();
}
