//! The picture the panel shows while the recovery gets ready: the same
//! moving field the About page has behind it, with the wordmark in the
//! colour the field gives it at each place, and a line saying that the
//! phone is starting. It runs on a thread of its own while the partitions
//! are mounted and the phone's programs started, and gives the panel back
//! before the pages are opened.

use anyhow::{Context, Result};
use skyward_config::DeviceConfig;
use skyward_ui::font::Fonts;
use skyward_ui::glow::{self, Dither, GlowClock, Layer};
use skyward_ui::surface::kms::KmsSurface;
use skyward_ui::theme::Theme;
use skyward_ui::{widgets, Canvas, Lang, Rgba, Surface};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The field is sampled this many pixels apart, as the About page samples
/// it, and filled in between.
const CELL: usize = 24;

/// How long the wordmark takes to come out of the field.
const WORD_MARK: f32 = 900.0;

/// The wordmark's size, as the About page draws it, and where its capitals
/// stand, as a share of the panel's height.
const WORDMARK_DP: f32 = 42.0;
const WORDMARK_AT: f32 = 0.47;

/// The slowest the picture moves, so that the boot it covers is not made
/// slower still.
const FRAME: Duration = Duration::from_millis(33);

/// How long the picture is shown at the least, however quickly the recovery
/// gets ready: on a phone that mounts its partitions in a tenth of a second
/// the picture would otherwise be gone before it is seen.
const MIN_SHOW: Duration = Duration::from_millis(1200);

/// A picture being shown, ended by [`Splash::stop`] or with this value.
pub struct Splash {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Option<KmsSurface>>>,
    /// When the picture went on the panel.
    started: Instant,
}

impl Splash {
    /// Show the picture while the recovery gets ready. Where the panel or
    /// the typeface cannot be had, none is shown and the pages come up as
    /// they would have.
    pub fn start(cfg: &DeviceConfig, font: &Path, mono: &Path) -> Option<Splash> {
        let stop = Arc::new(AtomicBool::new(false));
        let taken = stop.clone();
        let lang = cfg.device.lang.as_deref().and_then(|tag| Lang::for_locale(tag).ok()).unwrap_or(Lang::En);
        let brightness = cfg.display.brightness.unwrap_or(0.5);
        let density = cfg.display.density;
        // The pages' own theme decides the field the wordmark is inked
        // from: the light one their default has, or the dark one chosen
        // since, read before the picture starts.
        let dark = crate::settings::read().ok().and_then(|kept| kept.settings().map(|s| s.dark)).unwrap_or(false);
        let (font, mono) = (font.to_path_buf(), mono.to_path_buf());
        let started = Instant::now();
        let thread = std::thread::Builder::new()
            .name("splash".into())
            .spawn(move || match show(lang, brightness, density, dark, &font, &mono, &taken) {
                Ok(panel) => panel,
                Err(e) => {
                    let _ = skyward_sys::kmsg(&format!("the opening picture did not show: {e:#}"));
                    None
                }
            })
            .ok()?;
        Some(Splash { stop, thread: Some(thread), started })
    }

    /// Give the panel back, and wait until the picture has let go of it. The
    /// picture is left up for [`MIN_SHOW`] however little there was to wait
    /// for, so that it is seen. The panel comes back as the picture left
    /// it, showing its last frame, for the pages to take over without the
    /// panel going dark in between.
    pub fn stop(mut self) -> Option<KmsSurface> {
        let left = MIN_SHOW.saturating_sub(self.started.elapsed());
        if !left.is_zero() {
            std::thread::sleep(left);
        }
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().and_then(|thread| thread.join().ok()).flatten()
    }

    /// Ends the picture where the recovery ends before the pages open.
    fn end(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Splash {
    fn drop(&mut self) {
        self.end();
    }
}

/// Open the panel and the typeface, light the panel, and draw until told to
/// stop, giving the panel back as it stands, its last frame on it.
fn show(lang: Lang, brightness: f32, density: u32, dark: bool, font: &PathBuf, mono: &PathBuf, stop: &AtomicBool) -> Result<Option<KmsSurface>> {
    let mut surface = KmsSurface::open("/dev/dri/card0").context("open the panel")?;
    let (w, h) = surface.size();
    let fonts = Fonts::load(font, 0, mono).context("load the typeface")?;
    // The bootloader leaves the panel lit; a panel it left dark is lit
    // here, at the level the first start of the pages uses. Why it could
    // not be lit goes to the kernel log: a picture on a dark panel is none.
    match crate::ui::Backlight::open() {
        Ok(mut light) => {
            if let Err(e) = light.show(Some(brightness)) {
                let _ = skyward_sys::kmsg(&format!("the panel was not lit for the opening picture: {e:#}"));
            }
        }
        Err(e) => {
            let _ = skyward_sys::kmsg(&format!("no backlight was found for the opening picture: {e:#}"));
        }
    }
    let mut picture = Picture::new(w, h, density, dark, lang, &fonts);
    let _ = skyward_sys::kmsg(&format!("the opening picture is on the panel, {w}x{h}"));
    let mut canvas = Canvas::new(w, h);
    let start = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let began = Instant::now();
        picture.draw(&mut canvas, &fonts, start.elapsed().as_millis() as u64);
        surface.present(&canvas).context("show the opening picture")?;
        let spent = began.elapsed();
        if spent < FRAME {
            std::thread::sleep(FRAME - spent);
        }
    }
    // The last frame's flip is waited for, so the pages take a panel at rest
    // with its last picture on it.
    surface.settle().context("show the opening picture")?;
    Ok(Some(surface))
}

/// Draw one frame of the picture into `out` as a PNG, at `at` milliseconds
/// after it began, for looking at it away from a panel.
#[allow(clippy::too_many_arguments)]
pub fn frame(out: &Path, lang: Lang, density: u32, dark: bool, w: u32, h: u32, at: u64, font: &Path, mono: &Path) -> Result<()> {
    let fonts = Fonts::load(font, 0, mono).context("load the typeface")?;
    let mut picture = Picture::new(w, h, density, dark, lang, &fonts);
    let mut canvas = Canvas::new(w, h);
    picture.draw(&mut canvas, &fonts, at);
    let mut png = Vec::new();
    canvas.write_png(&mut png).context("make the picture a PNG")?;
    std::fs::write(out, png).with_context(|| format!("write {}", out.display()))
}

/// Draw the wordmark's coverage alone on white, for seeing whether the
/// typeface or its capture cuts any of it.
pub fn sprite_frame(out: &Path, density: u32, dark: bool, w: u32, h: u32, font: &Path, mono: &Path) -> Result<()> {
    let fonts = Fonts::load(font, 0, mono).context("load the typeface")?;
    let px_per_dp = (density.max(1) as f32) / 160.0;
    let theme = match dark {
        true => Theme::dark(w, h, px_per_dp),
        false => Theme::light(w, h, px_per_dp),
    };
    let px = theme.m.dp(WORDMARK_DP);
    let cap = fonts.display.cap(px);
    let sprite = widgets::wordmark(&theme, &fonts, "Skyward-Galaxy", h as f32 * WORDMARK_AT - cap / 2.0, px);
    println!("sprite {}x{} at ({}, {}) px {px}", sprite.w, sprite.h, sprite.x, sprite.y);
    let mut canvas = Canvas::filled(w, h, Rgba::rgb(255, 255, 255));
    sprite.draw(&mut canvas, 0.0, 0.0, Rgba::rgb(0, 0, 0), 1.0);
    let mut png = Vec::new();
    canvas.write_png(&mut png).context("make the picture a PNG")?;
    std::fs::write(out, png).with_context(|| format!("write {}", out.display()))
}

/// Draw the wordmark at several sizes down one picture, for seeing which
/// of them the typeface draws whole.
pub fn sprite_strip(out: &Path, w: u32, sizes: &[f32], font: &Path, mono: &Path) -> Result<()> {
    let fonts = Fonts::load(font, 0, mono).context("load the typeface")?;
    let gap = 20.0;
    let rows: Vec<f32> = sizes.iter().map(|px| (px * 1.9).ceil()).collect();
    let h: u32 = rows.iter().sum::<f32>() as u32 + (gap * (sizes.len() + 1) as f32) as u32;
    let mut canvas = Canvas::filled(w, h, Rgba::rgb(255, 255, 255));
    let face = &fonts.display;
    let mut y = gap;
    for (px, row) in sizes.iter().zip(&rows) {
        let width = canvas.text_width(face, "Skyward-Galaxy", *px);
        canvas.text(face, "Skyward-Galaxy", (w as f32 - width) / 2.0, y + px * 1.2, *px, Rgba::rgb(0, 0, 0), None);
        y += row + gap;
    }
    let mut png = Vec::new();
    canvas.write_png(&mut png).context("make the picture a PNG")?;
    std::fs::write(out, png).with_context(|| format!("write {}", out.display()))
}

/// What the picture is drawn from, once the panel, the typeface and the
/// theme are known.
struct Picture {
    theme: Theme,
    clock: GlowClock,
    dither: Dither,
    /// The layer over the field holds nothing: the field is the picture.
    empty: Canvas,
    /// Where the wordmark stands and how big it is, drawn afresh each frame
    /// in the colour the field gives it so that no letter is cut.
    word_x: f32,
    word_baseline: f32,
    word_px: f32,
    dark: bool,
    lang: Lang,
}

impl Picture {
    fn new(w: u32, h: u32, density: u32, dark: bool, lang: Lang, fonts: &Fonts) -> Picture {
        let px_per_dp = (density.max(1) as f32) / 160.0;
        let theme = match dark {
            true => Theme::dark(w, h, px_per_dp),
            false => Theme::light(w, h, px_per_dp),
        };
        // A whole-pixel em, as the typeface is drawn wrong at some
        // fractional ones, where a letter can come out cut.
        let px = theme.m.dp(WORDMARK_DP).round();
        let face = &fonts.display;
        let width = Canvas::new(0, 0).text_width(face, "Skyward-Galaxy", px);
        let cap_top = h as f32 * WORDMARK_AT - face.cap(px) / 2.0;
        Picture {
            clock: GlowClock::new(theme.hero.glow, 0),
            dither: Dither::new(w as usize, h as usize),
            empty: Canvas::new(w, h),
            word_x: (w as f32 - width) / 2.0,
            word_baseline: cap_top + face.cap(px),
            word_px: px,
            theme,
            dark,
            lang,
        }
    }

    /// Draw the picture at `now` milliseconds after it began.
    fn draw(&mut self, canvas: &mut Canvas, fonts: &Fonts, now: u64) {
        let (w, h) = (canvas.w as f32, canvas.h as f32);
        let theme = self.theme;
        self.clock.advance(now);
        // The discs fill the whole panel: the band the field is placed in is
        // as tall as the screen.
        let field = glow::sample(&self.clock, canvas.w as usize, canvas.h as usize, CELL, 1.0, theme.pal.bg, 1.0);
        glow::paint(canvas, &field, 1.0, &Layer { canvas: &self.empty, runs: &[], offset: 0 }, &self.dither);
        // The wordmark, in the colour the field gives it where each of its
        // pixels stands, coming out of the field as the pages come up. It is
        // drawn rather than stamped from a sprite: a stamp cut the last
        // letter of the typeface short at some sizes.
        let alpha = ease(now as f32 / WORD_MARK);
        let look = &theme.hero.glow.ink;
        let (cols, rows, cell, under_them) = (field.cols, field.rows, field.cell.max(1), &field.rgb);
        let ink = |x: f32, y: f32| -> Rgba {
            let under = under_field(under_them, cols, rows, cell, x, y);
            let [r, g, b] = glow::ink(look, under);
            Rgba { r: r.clamp(0.0, 255.0) as u8, g: g.clamp(0.0, 255.0) as u8, b: b.clamp(0.0, 255.0) as u8, a: (255.0 * alpha) as u8 }
        };
        canvas.text_paint(&fonts.display, "Skyward-Galaxy", self.word_x, self.word_baseline, self.word_px, &ink, None);
        // What it is doing, in the language the pages will speak.
        let said = self.lang.pick(["正在启动…", "正在啟動…", "Starting…"]);
        let note = (110.0 * ease(((now as f32) - 500.0).max(0.0) / 900.0)) as u8;
        let grey = match self.dark {
            true => Rgba::rgb(255, 255, 255),
            false => Rgba::rgb(70, 70, 80),
        };
        canvas.text_center(&fonts.body, said, w / 2.0, h * 0.90, theme.m.dp(11.5), grey.with_alpha(note));
    }
}

/// The field's colour under pixel (`x`, `y`), filled in between its grid
/// points as the field itself is.
fn under_field(rgb: &[[f32; 3]], cols: usize, rows: usize, cell: usize, x: f32, y: f32) -> [f32; 3] {
    let gx = (x / cell as f32).clamp(0.0, (cols.saturating_sub(1)) as f32);
    let gy = (y / cell as f32).clamp(0.0, (rows.saturating_sub(1)) as f32);
    let (i0, j0) = (gx.floor() as usize, gy.floor() as usize);
    let (i1, j1) = ((i0 + 1).min(cols - 1), (j0 + 1).min(rows - 1));
    let (fx, fy) = (gx - i0 as f32, gy - j0 as f32);
    let at = |i: usize, j: usize| rgb[j * cols + i];
    std::array::from_fn(|c| {
        let a = at(i0, j0)[c] + (at(i1, j0)[c] - at(i0, j0)[c]) * fx;
        let b = at(i0, j1)[c] + (at(i1, j1)[c] - at(i0, j1)[c]) * fx;
        a + (b - a) * fy
    })
}

/// A number from 0 to 1 that starts and ends slowly.
fn ease(p: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}
