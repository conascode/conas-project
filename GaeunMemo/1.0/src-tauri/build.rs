fn main() {
    // Declaring the app's commands makes each one require an explicit
    // `allow-*` permission in capabilities/default.json.
    let attrs = tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
        "note_get",
        "note_save",
        "note_set_color",
        "note_set_font",
        "note_toggle_pin",
        "note_toggle_fold",
        "note_new_near",
        "note_close",
        "note_delete",
        "band_set",
        "list_get",
        "list_open_note",
        "list_new",
        "list_close",
        "open_list",
        "theme_set",
    ]));
    tauri_build::try_build(attrs).expect("tauri build script failed");
}
