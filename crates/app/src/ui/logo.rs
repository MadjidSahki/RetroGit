//! RetroGit's logo: window icon, title bar icon, About box. Raw RGBA made by
//! `scripts/make-icons.py` from the full logo.

pub static ICON_32: &[u8] = include_bytes!("../../assets/icon-32.rgba");
pub static ICON_256: &[u8] = include_bytes!("../../assets/icon-256.rgba");
pub static LOGO_128: &[u8] = include_bytes!("../../assets/logo-128.rgba");

/// Icon of the window (Dock, task bar).
pub fn window_icon() -> egui::IconData {
    egui::IconData {
        rgba: ICON_256.to_vec(),
        width: 256,
        height: 256,
    }
}

fn texture(ctx: &egui::Context, name: &str, rgba: &[u8], size: usize) -> egui::TextureHandle {
    let id = egui::Id::new(("retrogit_logo", name));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t;
    }
    let image = egui::ColorImage::from_rgba_unmultiplied([size, size], rgba);
    let t = ctx.load_texture(name, image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// 32x32 icon, drawn 16x16 in the title bar.
pub fn icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    texture(ctx, "retrogit_icon", ICON_32, 32)
}

/// The whole logo, for the About box.
pub fn logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    texture(ctx, "retrogit_logo", LOGO_128, 128)
}
