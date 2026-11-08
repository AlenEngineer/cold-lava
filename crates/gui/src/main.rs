//! cold-lava as a window. A first step: it asks the same engine as the terminal UI for a workspace's
//! notes and lists them. Views, the document view and embeds come from `cold-lava-views` as they
//! are extracted.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use cold_lava_engine::workspace;
use std::path::Path;

/// The workspace's notes, by path, for the window to list.
#[tauri::command]
fn notes(workspace: String) -> Result<Vec<String>, String> {
    let v = workspace::scan(Path::new(&workspace)).map_err(|e| format!("{e:#}"))?;
    let mut notes: Vec<String> = v.notes.into_iter().map(|n| n.rel).collect();
    notes.sort();
    Ok(notes)
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![notes])
        .run(tauri::generate_context!())
        .expect("couldn't start the cold-lava window");
}
