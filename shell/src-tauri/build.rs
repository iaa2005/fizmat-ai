fn main() {
    // Команда guard объявлена в манифесте: тогда её можно разрешить страницам fizmat.ai
    // (capabilities/remote.json, «allow-guard»), а прочим сайтам — нет.
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&["guard"])),
    )
    .expect("tauri build");
}
