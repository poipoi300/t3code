#[cfg(target_os = "linux")]
use nsh_platform_linux as platform;
#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;
#[cfg(target_os = "windows")]
use nsh_platform_windows as platform;

/// `asset:/@<package>/<path>` files are read from `assets` beside the executable, which
/// `ui.ps1` fills in the packaged layout.
fn asset_base() -> Option<platform::AssetBase> {
    let dir = std::env::current_exe().ok()?.parent()?.join("assets");
    dir.is_dir().then_some(platform::AssetBase::Directory(dir))
}

fn main() -> impl std::process::Termination {
    platform::set_ui_sets(t3_ui::registries());
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<t3_ui::App>().url_schemes(["t3code-dev"])),
        platform::AppOptions {
            title: "T3 Code Dev".into(),
            app_id: "codes.t3.dev".into(),
            size: (1200, 800),
            asset_base: asset_base(),
            // One instance, as the TypeScript desktop's Electron lock: a second launch brings this
            // window forward. macOS runs one instance per bundle already.
            #[cfg(not(target_os = "macos"))]
            single_instance: true,
            ..Default::default()
        },
    )
}
