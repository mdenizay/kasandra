pub mod commands;
pub mod connection_url;
pub mod doctor;
pub mod drivers;
pub mod error;
pub mod models;
pub mod sql;
pub mod ssh;
pub mod state;
pub mod storage;
pub mod terminal;
pub mod tls;
pub mod transfer;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // A self-check that needs no window, so it still answers when the UI is the thing
    // that is broken.
    if std::env::args().any(|arg| arg == "--doctor") {
        attach_parent_console();
        std::process::exit(doctor::run());
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(state::AppState::default())
        .manage(terminal::TerminalManager::default())
        .on_window_event(|window, event| {
            // No ssh session should outlive the window that shows it.
            if let tauri::WindowEvent::Destroyed = event {
                use tauri::Manager;
                window.state::<terminal::TerminalManager>().close_all();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_profiles,
            commands::save_profile,
            commands::delete_profile,
            commands::parse_connection_url,
            commands::looks_like_connection_url,
            commands::connection_url_for_profile,
            commands::connect,
            commands::test_connection,
            commands::disconnect,
            commands::open_connection_ids,
            commands::execute_sql,
            commands::cancel_query,
            commands::list_databases,
            commands::list_schemas,
            commands::list_tables,
            commands::describe_table,
            commands::schema_columns,
            commands::create_statement,
            commands::use_database,
            commands::fetch_rows,
            commands::count_rows,
            commands::get_settings,
            commands::set_settings,
            commands::get_history,
            commands::add_history,
            commands::clear_history,
            commands::get_snippets,
            commands::set_snippets,
            commands::credential_store_available,
            commands::data_directory,
            commands::plan_row_edits,
            commands::apply_statements,
            commands::export_rows,
            commands::write_text_file,
            commands::dump_database,
            commands::run_script_file,
            commands::csv_preview,
            commands::csv_import,
            commands::terminal_open,
            commands::terminal_write,
            commands::terminal_resize,
            commands::terminal_close,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Kasandra");
}

/// A release build on Windows has no console of its own, so `--doctor` borrows the one it
/// was started from; otherwise its report would go nowhere.
#[cfg(windows)]
fn attach_parent_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}
