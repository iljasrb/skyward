use ab_glyph::{Font, FontRef, GlyphId, PxScale, ScaleFont, VariableFont};
use anyhow::{anyhow, Result};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

/// One weight instance of a typeface. MiSans ships as a variable font, so
/// every UI weight is the same file with a different `wght` coordinate.
pub struct Face {
    pub(crate) font: FontRef<'static>,
    upem: f32,
    height: f32,
    /// What drawing text in the face has worked out, kept: outlining a
    /// glyph of a variable font, its points moved for the weight, and
    /// rasterizing it cost more than all else a page draws.
    known: RefCell<Known>,
}

/// A glyph's ink: coverage from 0 to 255 for each pixel of a `w` × `h`
/// box whose top-left corner lies (`x`, `y`) from the whole pixel holding
/// the pen position.
pub(crate) struct Ink {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub cov: Vec<u8>,
}

/// Glyphs are drawn this many positions to a pixel each way, and their
/// ink kept for each.
pub(crate) const SUBPIXELS: f32 = 4.0;

/// So many glyph inks at the most are kept; past that, they are drawn
/// anew.
const GLYPHS_KEPT: usize = 16_384;

#[derive(Default)]
struct Known {
    /// Each character's glyph and advance, unscaled.
    chars: HashMap<char, (GlyphId, f32)>,
    /// The reference ideograph's ink extent at each em size.
    inks: HashMap<u32, (f32, f32)>,
    /// Each glyph's ink at each size and position within a pixel; nothing
    /// for a glyph without an outline.
    glyphs: HashMap<(u16, u32, u8, u8), Option<Rc<Ink>>>,
}

impl Face {
    fn new(data: &'static [u8], index: u32, weight: f32) -> Result<Self> {
        let mut font =
            FontRef::try_from_slice_and_index(data, index).map_err(|e| anyhow!("parse font: {e}"))?;
        font.set_variation(b"wght", weight);
        let upem = font.units_per_em().ok_or_else(|| anyhow!("font has no units-per-em"))?;
        let height = font.height_unscaled();
        Ok(Face { font, upem, height, known: RefCell::new(Known::default()) })
    }

    /// The glyph for `ch`, and how far it advances the pen at `scale`.
    pub(crate) fn glyph(&self, ch: char, scale: PxScale) -> (GlyphId, f32) {
        let (id, advance) = *self.known.borrow_mut().chars.entry(ch).or_insert_with(|| {
            let id = self.font.glyph_id(ch);
            (id, self.font.h_advance_unscaled(id))
        });
        (id, advance * scale.x / self.height)
    }

    /// The ink of glyph `id` at `scale`, drawn `qx` and `qy` quarters of a
    /// pixel right of and below a whole pixel.
    pub(crate) fn ink_of(&self, id: GlyphId, scale: PxScale, qx: u8, qy: u8) -> Option<Rc<Ink>> {
        let key = (id.0, scale.x.to_bits(), qx, qy);
        if let Some(ink) = self.known.borrow().glyphs.get(&key) {
            return ink.clone();
        }
        let at = ab_glyph::point(qx as f32 / SUBPIXELS, qy as f32 / SUBPIXELS);
        let ink = self.draw(id, scale, at).map(Rc::new);
        let mut known = self.known.borrow_mut();
        if known.glyphs.len() >= GLYPHS_KEPT {
            known.glyphs.clear();
        }
        known.glyphs.insert(key, ink.clone());
        ink
    }

    /// The ink of glyph `id` at `scale`, its pen `at` a fraction of a pixel
    /// right of and below a whole pixel.
    ///
    /// ab_glyph is not asked to draw it. It sizes the rasterizer to the ink's
    /// box exactly — the floor of the left and top edges to the ceiling of
    /// the right and bottom — so an edge that lands on a whole pixel
    /// coordinate lies on the rasterizer's last column, where its delta is
    /// dropped. Coverage is a running sum along the rasterizer's rows, so the
    /// leftover of a dropped delta carries to the end of that row and on into
    /// the next, and reads as a band of grey across the glyph. Placing the
    /// pen on quarters of a pixel lands an edge there often, so a pixel of
    /// margin on each side: every edge then falls strictly inside the
    /// rasterizer, and nothing is dropped.
    fn draw(&self, id: GlyphId, scale: PxScale, at: ab_glyph::Point) -> Option<Ink> {
        use ab_glyph::{OutlineCurve, Point};
        use ab_glyph_rasterizer::Rasterizer;

        let outline = self.font.outline(id)?;
        let factor = self.font.as_scaled(scale).scale_factor();
        let b = outline.px_bounds(factor, at);
        let (w, h) = (b.width() as u32, b.height() as u32);
        let (pw, ph) = (w as usize + 2, h as usize + 2);
        let corner = at - b.min + ab_glyph::point(1.0, 1.0);
        let place = |p: Point| Point { x: p.x * factor.horizontal, y: p.y * -factor.vertical } + corner;
        let mut raster = Rasterizer::new(pw, ph);
        for curve in &outline.curves {
            match *curve {
                OutlineCurve::Line(a, b) => raster.draw_line(place(a), place(b)),
                OutlineCurve::Quad(a, b, c) => raster.draw_quad(place(a), place(b), place(c)),
                OutlineCurve::Cubic(a, b, c, d) => raster.draw_cubic(place(a), place(b), place(c), place(d)),
            }
        }
        let mut cov = vec![0u8; (w * h) as usize];
        raster.for_each_pixel(|idx, a| {
            let (px, py) = (idx % pw, idx / pw);
            if (1..=w as usize).contains(&px) && (1..=h as usize).contains(&py) {
                cov[(py - 1) * w as usize + px - 1] = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        });
        Some(Ink { x: b.min.x as i32, y: b.min.y as i32, w, h, cov })
    }

    /// ab_glyph scales by ascent-minus-descent; the UI sizes text by the em,
    /// the same way Android `sp` values are defined.
    pub fn scale(&self, em_px: f32) -> PxScale {
        PxScale::from(em_px * self.height / self.upem)
    }

    /// Whether the typeface draws `ch` itself.
    pub fn has(&self, ch: char) -> bool {
        self.font.glyph_id(ch).0 != 0
    }

    /// Vertical ink extent of the reference glyph 「｜」 relative to the
    /// baseline (negative is above). Rows centre text on this, as the stock
    /// settings do.
    pub fn ink(&self, em_px: f32) -> (f32, f32) {
        if let Some(&ink) = self.known.borrow().inks.get(&em_px.to_bits()) {
            return ink;
        }
        let ink = self.bounds('｜', em_px);
        self.known.borrow_mut().inks.insert(em_px.to_bits(), ink);
        ink
    }

    /// The line text is set on, above and below the baseline (negative is
    /// above): the face's ascent and descent, which a text field's cursor
    /// spans.
    pub fn line(&self, em_px: f32) -> (f32, f32) {
        let k = em_px / self.upem;
        (-self.font.ascent_unscaled() * k, -self.font.descent_unscaled() * k)
    }

    /// Height of a capital letter above the baseline, for Latin lettering
    /// placed by its cap line.
    pub fn cap(&self, em_px: f32) -> f32 {
        -self.bounds('H', em_px).0
    }

    /// Where the ink of `text` set at `em_px` starts and ends along the
    /// line, from where the pen starts: its glyphs' leftmost ink and their
    /// rightmost, from their outlines, unrounded. The pen moves as a run of
    /// text is drawn, kerning and all.
    pub fn ink_span(&self, text: &str, em_px: f32) -> (f32, f32) {
        let scale = self.scale(em_px);
        let scaled = self.font.as_scaled(scale);
        let k = em_px / self.upem;
        let (mut pen, mut prev, mut span) = (0.0, None, (f32::MAX, f32::MIN));
        for ch in text.chars() {
            let (id, advance) = self.glyph(ch, scale);
            if let Some(p) = prev {
                pen += scaled.kern(p, id);
            }
            if let Some(o) = self.font.outline(id) {
                span = (span.0.min(pen + o.bounds.min.x * k), span.1.max(pen + o.bounds.max.x * k));
            }
            pen += advance;
            prev = Some(id);
        }
        span
    }

    /// Where the ink of `text` set at `em_px` starts and ends above and below
    /// the baseline (negative is above), the glyphs' outlines taken together;
    /// none while there is nothing in it that draws.
    pub fn ink_v(&self, text: &str, em_px: f32) -> Option<(f32, f32)> {
        let mut span: Option<(f32, f32)> = None;
        for ch in text.chars() {
            let glyph = self.font.glyph_id(ch).with_scale_and_position(self.scale(em_px), ab_glyph::point(0.0, 0.0));
            let Some(outline) = self.font.outline_glyph(glyph) else { continue };
            let b = outline.px_bounds();
            span = Some(match span {
                None => (b.min.y, b.max.y),
                Some((top, bottom)) => (top.min(b.min.y), bottom.max(b.max.y)),
            });
        }
        span
    }

    fn bounds(&self, ch: char, em_px: f32) -> (f32, f32) {
        let glyph = self.font.glyph_id(ch).with_scale_and_position(self.scale(em_px), ab_glyph::point(0.0, 0.0));
        // A typeface that does not carry a character draws nothing for it,
        // rather than taking the recovery down: a file name or a network's
        // name may hold anything.
        let Some(og) = self.font.outline_glyph(glyph) else { return (0.0, 0.0) };
        let b = og.px_bounds();
        (b.min.y, b.max.y)
    }
}

/// `wght` per text role, fitted on Xiaomi 17 Ultra screenshots:
/// for each string, the size and weight whose rendering matches the ink
/// width, height and colour-normalised coverage (`skyward-preview --match`).
pub struct Weights {
    pub title: f32,
    pub heading: f32,
    pub label: f32,
    pub body: f32,
    pub value: f32,
    pub badge: f32,
    pub status: f32,
    pub strong: f32,
    pub speed: f32,
    pub display: f32,
}

pub const STOCK_WEIGHTS: Weights = Weights {
    title: 322.0,
    heading: 365.0,
    label: 370.0,
    body: 315.0,
    value: 320.0,
    badge: 505.0,
    status: 485.0,
    strong: 545.0,
    speed: 680.0,
    display: 480.0,
};

pub struct Fonts {
    /// Large page title.
    pub title: Face,
    /// Row, card and grid-value titles.
    pub heading: Face,
    /// Section captions.
    pub label: Face,
    /// Subtitles, placeholders and explanatory text.
    pub body: Face,
    /// Trailing values.
    pub value: Face,
    pub badge: Face,
    pub status: Face,
    /// Digits inside the battery pill.
    pub strong: Face,
    /// How fast data goes, in the status bar.
    pub speed: Face,
    /// The About page wordmark.
    pub display: Face,
    /// The terminal's monospaced face, for the characters it has.
    pub mono: Face,
    /// What the UI typeface says about itself.
    pub info: FontInfo,
    /// What the monospaced typeface says about itself.
    pub mono_info: FontInfo,
    data: &'static [u8],
    index: u32,
}

/// A typeface's own name, version and copyright, from its name table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontInfo {
    pub family: String,
    pub version: String,
    /// The copyright notice, when the font carries one; some leave it to the
    /// licence file that comes with them.
    pub copyright: Option<String>,
}

impl FontInfo {
    fn read(data: &[u8], index: u32) -> Result<FontInfo> {
        let face = ttf_parser::Face::parse(data, index).map_err(|e| anyhow!("parse font: {e}"))?;
        let name = |id: u16| -> Option<String> {
            face.names().into_iter().filter(|n| n.name_id == id && n.is_unicode()).find_map(|n| n.to_string())
        };
        let needed = |id: u16| name(id).ok_or_else(|| anyhow!("the font names no entry {id}"));
        let version = needed(ttf_parser::name_id::VERSION)?;
        Ok(FontInfo {
            family: needed(ttf_parser::name_id::FAMILY)?,
            // "Version 4.009" names the version twice in a row labelled version.
            version: version.strip_prefix("Version ").unwrap_or(&version).to_string(),
            copyright: name(ttf_parser::name_id::COPYRIGHT_NOTICE),
        })
    }
}

/// Read a font file for the life of the process: the faces borrow its bytes.
fn leak(path: &Path) -> Result<&'static [u8]> {
    let bytes = std::fs::read(path).map_err(|e| anyhow!("read font {}: {e}", path.display()))?;
    Ok(Box::leak(bytes.into_boxed_slice()))
}

impl Fonts {
    /// The UI typeface from `path` (face `index` of a collection) and the
    /// terminal's monospaced one from `mono`.
    pub fn load(path: impl AsRef<Path>, index: u32, mono: impl AsRef<Path>) -> Result<Self> {
        Self::load_with(path, index, mono, &STOCK_WEIGHTS)
    }

    pub fn load_with(path: impl AsRef<Path>, index: u32, mono: impl AsRef<Path>, w: &Weights) -> Result<Self> {
        // One copy of the bytes backs every weight instance.
        let data = leak(path.as_ref())?;
        let mono_data = leak(mono.as_ref())?;
        let face = |wght: f32| Face::new(data, index, wght);
        Ok(Fonts {
            title: face(w.title)?,
            heading: face(w.heading)?,
            label: face(w.label)?,
            body: face(w.body)?,
            value: face(w.value)?,
            badge: face(w.badge)?,
            status: face(w.status)?,
            strong: face(w.strong)?,
            speed: face(w.speed)?,
            display: face(w.display)?,
            // A single-weight file: the coordinate asks for the weight it is.
            mono: Face::new(mono_data, 0, 400.0)?,
            info: FontInfo::read(data, index)?,
            mono_info: FontInfo::read(mono_data, 0)?,
            data,
            index,
        })
    }

    /// An extra instance at an arbitrary `wght` coordinate.
    pub fn face(&self, weight: f32) -> Result<Face> {
        Face::new(self.data, self.index, weight)
    }

    /// `(tag, min, default, max)` for every variation axis.
    pub fn axes(&self) -> Vec<(String, f32, f32, f32)> {
        self.title
            .font
            .variations()
            .into_iter()
            .map(|a| (String::from_utf8_lossy(&a.tag).into_owned(), a.min_value, a.default_value, a.max_value))
            .collect()
    }
}

/// The typefaces the recovery image carries, from the repository.
#[cfg(test)]
pub(crate) fn bundled() -> Fonts {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../resources/fonts");
    Fonts::load(format!("{dir}/MiSansVF.ttf"), 0, format!("{dir}/KirkMono-Regular.ttf")).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_as_tall_as_the_stock_search_fields_cursor() {
        // Xiaomi 17 Ultra, 3 px a dp, the search field's 16.9 dp text: its
        // placeholder's ink 47 px tall, its cursor 69 px, measured on screen.
        let f = bundled();
        let em = 16.9 * 3.0;
        let (ink_top, ink_bottom) = f.heading.ink(em);
        let (top, bottom) = f.heading.line(em);
        assert!((ink_bottom - ink_top - 47.0).abs() <= 2.0, "ink {ink_top}..{ink_bottom}");
        assert!((bottom - top - 69.0).abs() <= 2.0, "line {top}..{bottom}");
    }

    /// A glyph's ink holds as much of itself whichever quarter of a pixel
    /// the pen sits on: the shift moves its edges by a fraction of a pixel,
    /// not its mass. It used to gain far more: ab_glyph sizes its rasterizer
    /// to the ink's box exactly, so an edge landing on the last column has
    /// its delta dropped and coverage, a running sum along the rows, takes
    /// the leftover across the rest of the row. Every fourth pen position
    /// landed one there, which on the phone read as a band of grey behind the
    /// `y` of the about page's wordmark.
    #[test]
    fn the_ink_of_a_glyph_holds_its_mass_at_every_quarter() {
        let f = bundled();
        let scale = f.display.scale(132.3);
        for ch in "Skyward 0123456789".chars().filter(|&ch| ch != ' ') {
            let (id, _) = f.display.glyph(ch, scale);
            let masses: Vec<f32> = (0..SUBPIXELS as u8)
                .flat_map(|qy| (0..SUBPIXELS as u8).map(move |qx| (qx, qy)))
                .map(|(qx, qy)| {
                    let ink = f.display.ink_of(id, scale, qx, qy).expect("every bundled glyph is outlined");
                    ink.cov.iter().map(|&c| f32::from(c)).sum::<f32>()
                })
                .collect();
            let (least, most) = masses.iter().fold((f32::MAX, 0.0f32), |(l, m), &v| (l.min(v), m.max(v)));
            assert!(most <= least * 1.1, "{ch}: ink of {least} at one quarter of a pixel, {most} at another");
        }
    }
}
