fn main() {
    // Команды объявлены в манифесте: тогда их можно разрешить страницам fizmat.ai
    // (capabilities/remote.json, «allow-…»), а прочим сайтам — нет.
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&["guard", "code_status", "code_pair", "code_add_root", "code_remove_root", "code_ticket", "quick_ask", "quick_hide"])),
    )
    .expect("tauri build");
}
