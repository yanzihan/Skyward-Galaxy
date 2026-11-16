//! Draws the wordmark at two sizes, to see whether the typeface itself
//! renders a letter differently at one of them.

use skyward_ui::font::Fonts;
use skyward_ui::{Canvas, Rgba};
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fonts = Fonts::load(root.join("resources/fonts/MiSansVF.ttf"), 0, root.join("resources/fonts/KirkMono-Regular.ttf"))?;
    for (name, px) in [("98", 98.4375f32), ("113", 113.0f32)] {
        let mut c = Canvas::filled(1400, 300, Rgba::rgb(255, 255, 255));
        let face = &fonts.display;
        let w = c.text_width(face, "Skyward-Galaxy", px);
        c.text(face, "Skyward-Galaxy", (1400.0 - w) / 2.0, 200.0, px, Rgba::rgb(0, 0, 0), None);
        let mut png = Vec::new();
        c.write_png(&mut png)?;
        let at = format!("/tmp/wordmark-{name}.png");
        std::fs::write(&at, png)?;
        println!("{at} width {w}");
    }
    Ok(())
}
