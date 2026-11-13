use nsh_platform_gpui as platform;

fn main() {
    platform::set_ui_sets(t3_ui::registries());
    let asset_base = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("assets")))
        .filter(|dir| dir.is_dir())
        .map(platform::AssetBase::Directory);
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<t3_ui::App>()),
        platform::AppOptions {
            title: "T3 Code Dev / GPUI".into(),
            app_id: "codes.t3.dev".into(),
            size: (1200, 800),
            asset_base,
            ..Default::default()
        },
    );
}
