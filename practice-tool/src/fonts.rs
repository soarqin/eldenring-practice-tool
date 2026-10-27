//! Rasterize requested fallback glyphs with the original ImGui font builder.
//! Font links retain their Windows order. Font files are mapped on demand,
//! without copying or preloading their contents into the atlas.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr;
use std::time::{Duration, Instant};

use hudhook::tracing::warn;
use hudhook::RenderContext;
use imgui::internal::RawCast;
use imgui::{sys, Context, TextureId, Ui};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Registry::*;

static REQUESTS: Lazy<Mutex<BTreeSet<(usize, char)>>> = Lazy::new(Mutex::default);

// One bounded glyph pool, allocated only after the first missing character.
// The resulting RGBA atlas is 2048 x 2048 (16 MiB), regardless of font count.
const ATLAS_WIDTH: i32 = 2048;
const POOL_WIDTH: u32 = 2046;
const POOL_HEIGHT: u32 = 1024;
// hudhook replaces the whole texture and keeps every upload buffer, so glyphs
// added together share one upload once requests settle. Until then they are
// drawn from still transparent pool space.
const UPLOAD_SETTLE: Duration = Duration::from_millis(200);
const UPLOAD_MAX_DELAY: Duration = Duration::from_secs(1);

pub(crate) fn request_text(ui: &Ui, text: &str) {
    let font = ui.current_font() as *const imgui::Font as *mut sys::ImFont;
    let missing = text.chars().filter(|&c| {
        !c.is_control() && unsafe { sys::ImFont_FindGlyphNoFallback(font, c as u32).is_null() }
    });
    REQUESTS.lock().extend(missing.map(|c| (font as usize, c)));
}

#[derive(Debug, Eq, PartialEq)]
struct FontLink {
    file: String,
    face: String,
}

fn parse_links(data: &[u16]) -> Vec<FontLink> {
    let mut names = Vec::new();
    for entry in data.split(|c| *c == 0).filter(|s| !s.is_empty()) {
        let entry = String::from_utf16_lossy(entry);
        // The list specifies font files and faces; use ImGui's pixel size and
        // sampling parameters, as in the original renderer.
        let mut parts = entry.split(',').map(str::trim);
        if let (Some(file), Some(face)) = (parts.next(), parts.next()) {
            if !file.is_empty()
                && !face.is_empty()
                && !names.iter().any(|link: &FontLink| link.face.eq_ignore_ascii_case(face))
            {
                names.push(FontLink { file: file.into(), face: face.into() });
            }
        }
    }
    names
}

fn font_links() -> Vec<FontLink> {
    let mut size = 0;
    let key = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\FontLink\\SystemLink");
    unsafe {
        if RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            w!("Segoe UI"),
            RRF_RT_REG_MULTI_SZ,
            None,
            None,
            Some(&mut size),
        )
        .is_err()
            || size == 0
            || size > 65536
            || size % 2 != 0
        {
            return Vec::new();
        }
        let mut data = vec![0u16; size as usize / 2];
        if RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            w!("Segoe UI"),
            RRF_RT_REG_MULTI_SZ,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .is_err()
        {
            return Vec::new();
        }
        parse_links(&data)
    }
}

struct Face {
    dc: HDC,
    font: HFONT,
    previous: HGDIOBJ,
}

impl Face {
    fn new(name: &str) -> Option<Self> {
        let name: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return None;
            }
            let font = CreateFontW(
                -18,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_TT_ONLY_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                0,
                PCWSTR(name.as_ptr()),
            );
            if font.is_invalid() {
                let _ = DeleteDC(dc);
                return None;
            }
            let previous = SelectObject(dc, HGDIOBJ(font.0));
            let face = Self { dc, font, previous };
            let mut actual = [0u16; 128];
            let count = GetTextFaceW(dc, Some(&mut actual));
            if count <= 0
                || !String::from_utf16_lossy(&actual[..count as usize - 1])
                    .eq_ignore_ascii_case(&String::from_utf16_lossy(&name[..name.len() - 1]))
            {
                return None;
            }
            Some(face)
        }
    }

    fn has_glyph(&self, character: char) -> bool {
        // Game names use BMP characters. Do not treat surrogate halves as
        // glyphs.
        let Ok(character) = u16::try_from(character as u32) else {
            return false;
        };
        let mut index = 0xffff;
        unsafe {
            let result = GetGlyphIndicesW(
                self.dc,
                PCWSTR(&character),
                1,
                &mut index,
                GGI_MARK_NONEXISTING_GLYPHS,
            );
            result != GDI_ERROR as u32 && index != 0xffff && index != 0
        }
    }

    fn name_table(&self) -> Option<Vec<u8>> {
        let tag = u32::from_le_bytes(*b"name");
        unsafe {
            let size = GetFontData(self.dc, tag, 0, None, 0);
            if !(6..=1024 * 1024).contains(&size) {
                return None;
            }
            let mut data = vec![0; size as usize];
            (GetFontData(self.dc, tag, 0, Some(data.as_mut_ptr().cast()), size) == size)
                .then_some(data)
        }
    }
}

impl Drop for Face {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.font.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

struct Glyph {
    character: char,
    metrics: sys::ImFontGlyph,
    width: u32,
    height: u32,
    bitmap: Vec<u8>,
}

struct MappedFont {
    _file: File,
    mapping: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    len: usize,
    index: i32,
}

// The view is read-only, stays mapped for the full borrow, and may be moved
// between renderer threads. No mutable references to its contents are exposed.
unsafe impl Send for MappedFont {}
unsafe impl Sync for MappedFont {}

impl MappedFont {
    fn open(link: &FontLink, face: &Face) -> Option<Self> {
        let windows = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
        let path = Path::new(&windows).join("Fonts").join(&link.file);
        let file = File::open(path).ok()?;
        let len = usize::try_from(file.metadata().ok()?.len()).ok()?;
        if !(12..=64 * 1024 * 1024).contains(&len) {
            return None;
        }
        unsafe {
            let mapping = CreateFileMappingW(
                HANDLE(file.as_raw_handle()),
                None,
                PAGE_READONLY,
                0,
                0,
                PCWSTR::null(),
            )
            .ok()?;
            let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, len);
            if view.Value.is_null() {
                let _ = CloseHandle(mapping);
                return None;
            }
            let mut mapped = Self { _file: file, mapping, view, len, index: 0 };
            mapped.index = font_index(mapped.data(), &face.name_table()?)?;
            Some(mapped)
        }
    }

    fn data(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.view.Value.cast(), self.len) }
    }

    fn rasterize(&self, destination: *mut sys::ImFont, characters: &[char]) -> Vec<Glyph> {
        let ranges: Vec<sys::ImWchar> = characters
            .iter()
            .flat_map(|&c| [c as sys::ImWchar, c as sys::ImWchar])
            .chain(Some(0))
            .collect();
        let mut atlas = imgui::SharedFontAtlas::create();
        unsafe {
            let raw = atlas.as_ptr_mut();
            (*raw).Flags =
                (sys::ImFontAtlasFlags_NoMouseCursors | sys::ImFontAtlasFlags_NoBakedLines) as i32;
            let mut config = *(*destination).ConfigData;
            config.FontData = self.view.Value;
            config.FontDataSize = self.len as i32;
            config.FontNo = self.index;
            config.SizePixels = (*destination).FontSize;
            config.GlyphRanges = ranges.as_ptr();
            config.MergeMode = false;
            config.DstFont = ptr::null_mut();
            // AddFont otherwise copies the complete TTF. Lend the mapped data
            // through the ownership-taking path, then immediately mark the
            // stored config as borrowed before building or destroying it.
            config.FontDataOwnedByAtlas = true;
            let font = sys::ImFontAtlas_AddFont(raw, &config);
            (*(*raw).ConfigData.Data).FontDataOwnedByAtlas = false;
            let mut pixels = ptr::null_mut();
            let (mut width, mut height) = (0, 0);
            sys::ImFontAtlas_GetTexDataAsAlpha8(
                raw,
                &mut pixels,
                &mut width,
                &mut height,
                ptr::null_mut(),
            );
            if pixels.is_null() || width <= 0 || height <= 0 {
                return Vec::new();
            }
            let pixels = std::slice::from_raw_parts(pixels, width as usize * height as usize);
            // This is the same baseline used by ImGui's MergeMode builder.
            let baseline = (*destination).Ascent.round() - (*font).Ascent.round();
            characters
                .iter()
                .filter_map(|&character| {
                    let glyph = sys::ImFont_FindGlyphNoFallback(font, character as u32);
                    if glyph.is_null() {
                        return None;
                    }
                    let mut metrics = *glyph;
                    let x = (metrics.U0 * width as f32).round() as usize;
                    let y = (metrics.V0 * height as f32).round() as usize;
                    let w = ((metrics.U1 - metrics.U0) * width as f32).round() as usize;
                    let h = ((metrics.V1 - metrics.V0) * height as f32).round() as usize;
                    if x + w > width as usize || y + h > height as usize {
                        return None;
                    }
                    let mut bitmap = Vec::with_capacity(w * h);
                    for row in y..y + h {
                        bitmap.extend_from_slice(
                            &pixels[row * width as usize + x..row * width as usize + x + w],
                        );
                    }
                    metrics.Y0 += baseline;
                    metrics.Y1 += baseline;
                    Some(Glyph { character, metrics, width: w as u32, height: h as u32, bitmap })
                })
                .collect()
        }
    }
}

impl Drop for MappedFont {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.mapping);
        }
    }
}

fn be_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(offset..offset.checked_add(4)?)?.try_into().ok()?))
}

fn name_table(data: &[u8], offset: usize) -> Option<&[u8]> {
    let count = u16::from_be_bytes(data.get(offset + 4..offset + 6)?.try_into().ok()?);
    if count > 512 {
        return None;
    }
    for index in 0..usize::from(count) {
        let at = offset.checked_add(12 + index * 16)?;
        let start = be_u32(data, at + 8)? as usize;
        let len = be_u32(data, at + 12)? as usize;
        let table = data.get(start..start.checked_add(len)?)?;
        if data.get(at..at + 4)? == b"name" {
            return Some(table);
        }
    }
    None
}

fn font_index(data: &[u8], selected_name: &[u8]) -> Option<i32> {
    if data.get(..4)? != b"ttcf" {
        return (name_table(data, 0)? == selected_name).then_some(0);
    }
    let count = be_u32(data, 8)?;
    if count > 64 {
        return None;
    }
    (0..count).find_map(|index| {
        let offset = be_u32(data, 12 + index as usize * 4)? as usize;
        (name_table(data, offset)? == selected_name).then_some(index as i32)
    })
}

#[derive(Default)]
struct Shelf {
    x: u32,
    y: u32,
    height: u32,
}

impl Shelf {
    fn allocate(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        let (width, height) = (width.checked_add(2)?, height.checked_add(2)?);
        if width > POOL_WIDTH || height > POOL_HEIGHT {
            return None;
        }
        if self.x + width > POOL_WIDTH {
            self.x = 0;
            self.y += self.height;
            self.height = 0;
        }
        if self.y + height > POOL_HEIGHT {
            return None;
        }
        let position = (self.x + 1, self.y + 1);
        self.x += width;
        self.height = self.height.max(height);
        Some(position)
    }
}

#[derive(Default)]
pub(crate) struct FontFallback {
    links: Option<Vec<FontLink>>,
    sources: BTreeMap<usize, Option<MappedFont>>,
    unavailable: BTreeSet<(usize, char)>,
    pool: Option<i32>,
    shelf: Shelf,
    /// Rebuilding the atlas resets its texture ID to the original font
    /// texture, which is too small for the pool.
    texture: Option<TextureId>,
    /// First and latest glyph additions that are not uploaded yet.
    pending: Option<(Instant, Instant)>,
    /// The atlas layout changed; upload before it is drawn.
    rebuilt: bool,
}

impl FontFallback {
    pub(crate) fn reset(&mut self) {
        REQUESTS.lock().clear();
        *self = Self::default();
    }

    pub(crate) fn update(&mut self, ctx: &mut Context, renderer: &mut dyn RenderContext) {
        self.update_at(ctx, renderer, Instant::now());
    }

    fn update_at(&mut self, ctx: &mut Context, renderer: &mut dyn RenderContext, now: Instant) {
        let requested = std::mem::take(&mut *REQUESTS.lock());
        if requested.is_empty() && self.pending.is_none() && !self.rebuilt {
            return;
        }
        let links = self.links.get_or_insert_with(font_links);
        let mut faces = BTreeMap::<usize, Option<Face>>::new();
        let mut batches = BTreeMap::<(usize, usize), Vec<char>>::new();
        let mut changed = BTreeSet::new();
        for (font, character) in requested {
            if self.unavailable.contains(&(font, character)) {
                continue;
            }
            let font = font as *mut sys::ImFont;
            if unsafe { !sys::ImFont_FindGlyphNoFallback(font, character as u32).is_null() } {
                continue;
            }
            let source = links.iter().enumerate().find_map(|(index, link)| {
                let face = faces.entry(index).or_insert_with(|| Face::new(&link.face)).as_ref()?;
                if !face.has_glyph(character) {
                    return None;
                }
                self.sources
                    .entry(index)
                    .or_insert_with(|| MappedFont::open(link, face))
                    .as_ref()?;
                Some(index)
            });
            let Some(source) = source else {
                self.unavailable.insert((font as usize, character));
                continue;
            };
            batches.entry((font as usize, source)).or_default().push(character);
        }
        for ((font, source), characters) in batches {
            let font = font as *mut sys::ImFont;
            let glyphs = self.sources[&source].as_ref().unwrap().rasterize(font, &characters);
            // GDI can report a glyph that ImGui's builder does not produce.
            // Visible text requests it every frame; do not rasterize it again.
            for &character in &characters {
                if !glyphs.iter().any(|glyph| glyph.character == character) {
                    self.unavailable.insert((font as usize, character));
                }
            }
            for glyph in glyphs {
                let character = glyph.character;
                let (width, height) = (glyph.width, glyph.height);
                let position = self.shelf.allocate(width.max(1), height.max(1)).or_else(|| {
                    // Drop old fallback glyphs rather than permanently losing
                    // new characters when changing locales or cycling font
                    // sizes. Visible text requests evicted glyphs again.
                    self.shelf = Shelf::default();
                    ctx.fonts().clear_tex_data();
                    ctx.fonts().build_rgba32_texture();
                    self.rebuilt = true;
                    self.shelf.allocate(width.max(1), height.max(1))
                });
                let Some((x, y)) = position else {
                    self.unavailable.insert((font as usize, character));
                    continue;
                };
                let atlas = ctx.fonts();
                if self.pool.is_none() {
                    atlas.tex_desired_width = ATLAS_WIDTH;
                    self.pool = Some(unsafe {
                        sys::ImFontAtlas_AddCustomRectRegular(
                            atlas.raw_mut(),
                            POOL_WIDTH as i32,
                            POOL_HEIGHT as i32,
                        )
                    });
                    atlas.clear_tex_data();
                    atlas.build_rgba32_texture();
                    self.rebuilt = true;
                }
                unsafe {
                    let raw = atlas.raw_mut();
                    let rect = &*sys::ImFontAtlas_GetCustomRectByIndex(raw, self.pool.unwrap());
                    let (x, y) = (x + u32::from(rect.X), y + u32::from(rect.Y));
                    let tex_width = raw.TexWidth as u32;
                    let tex_height = raw.TexHeight as u32;
                    if rect.X == u16::MAX
                        || rect.Y == u16::MAX
                        || x + width > tex_width
                        || y + height > tex_height
                    {
                        self.unavailable.insert((font as usize, character));
                        warn!("Could not pack fallback glyphs into the font atlas");
                        continue;
                    }
                    let pixels = std::slice::from_raw_parts_mut(
                        raw.TexPixelsRGBA32.cast::<u8>(),
                        (tex_width * tex_height * 4) as usize,
                    );
                    for row in 0..height {
                        for column in 0..width {
                            let alpha = glyph.bitmap[(row * width + column) as usize];
                            let pixel = (((y + row) * tex_width + x + column) * 4) as usize;
                            pixels[pixel..pixel + 4].copy_from_slice(&[255, 255, 255, alpha]);
                        }
                    }
                    sys::ImFont_AddGlyph(
                        font,
                        ptr::null(),
                        character as u32,
                        glyph.metrics.X0,
                        glyph.metrics.Y0,
                        glyph.metrics.X1,
                        glyph.metrics.Y1,
                        x as f32 / tex_width as f32,
                        y as f32 / tex_height as f32,
                        (x + width) as f32 / tex_width as f32,
                        (y + height) as f32 / tex_height as f32,
                        glyph.metrics.AdvanceX,
                    );
                }
                changed.insert(font as usize);
            }
        }
        if !changed.is_empty() {
            self.pending = Some((self.pending.map_or(now, |(first, _)| first), now));
        }
        for font in changed {
            unsafe { sys::ImFont_BuildLookupTable(font as *mut sys::ImFont) };
        }
        let settled = self.pending.is_some_and(|(first, last)| {
            now.duration_since(last) >= UPLOAD_SETTLE
                || now.duration_since(first) >= UPLOAD_MAX_DELAY
        });
        if self.rebuilt || settled {
            let atlas = ctx.fonts();
            let texture = atlas.build_rgba32_texture();
            let result = match self.texture {
                Some(id) => renderer
                    .replace_texture(id, texture.data, texture.width, texture.height)
                    .map(|()| id),
                None => renderer.load_texture(texture.data, texture.width, texture.height),
            };
            match result {
                Ok(id) => {
                    self.texture = Some(id);
                    self.pending = None;
                    self.rebuilt = false;
                },
                Err(error) => warn!("Could not upload fallback glyphs: {error}"),
            }
        }
        if let Some(id) = self.texture {
            ctx.fonts().tex_id = id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct GlyphPixels {
        metrics: [u32; 5],
        size: (usize, usize),
        pixels: Vec<u8>,
    }

    fn glyph_pixels(
        atlas: &mut imgui::FontAtlas,
        font: *mut sys::ImFont,
        c: u32,
    ) -> Option<GlyphPixels> {
        unsafe {
            let glyph = sys::ImFont_FindGlyphNoFallback(font, c);
            if glyph.is_null() {
                return None;
            }
            let glyph = &*glyph;
            let texture = atlas.build_rgba32_texture();
            let x = (glyph.U0 * texture.width as f32).round() as usize;
            let y = (glyph.V0 * texture.height as f32).round() as usize;
            let w = ((glyph.U1 - glyph.U0) * texture.width as f32).round() as usize;
            let h = ((glyph.V1 - glyph.V0) * texture.height as f32).round() as usize;
            let mut pixels = Vec::with_capacity(w * h);
            for row in y..y + h {
                for column in x..x + w {
                    pixels.push(texture.data[(row * texture.width as usize + column) * 4 + 3]);
                }
            }
            Some(GlyphPixels {
                metrics: [
                    glyph.AdvanceX.to_bits(),
                    glyph.X0.to_bits(),
                    glyph.Y0.to_bits(),
                    glyph.X1.to_bits(),
                    glyph.Y1.to_bits(),
                ],
                size: (w, h),
                pixels,
            })
        }
    }

    fn original_glyphs(size: f32, data: &[u8], index: i32) -> BTreeMap<u32, GlyphPixels> {
        let characters: BTreeSet<_> =
            "传送宁姆格福王之卢恩暗月大剑中新文".chars().map(u32::from).collect();
        let ranges: Vec<_> = characters.iter().flat_map(|&c| [c, c]).chain(Some(0)).collect();
        let mut shared = imgui::SharedFontAtlas::create();
        let atlas = unsafe { &mut *(shared.as_ptr_mut() as *mut imgui::FontAtlas) };
        // Exact pre-change built-in font source: ComicMono, config: None.
        let id = atlas.add_font(&[imgui::FontSource::TtfData {
            data: include_bytes!("../../lib/data/ComicMono.ttf"),
            size_pixels: size,
            config: None,
        }]);
        let font = atlas.get_font(id).unwrap() as *const imgui::Font as *mut sys::ImFont;
        atlas.build_alpha8_texture();
        unsafe {
            let mut config = *(*font).ConfigData;
            config.FontData = data.as_ptr() as *mut _;
            config.FontDataSize = data.len() as i32;
            // The original AddFont path copies the source.
            config.FontDataOwnedByAtlas = false;
            config.FontNo = index;
            config.GlyphRanges = ranges.as_ptr();
            config.MergeMode = true;
            config.DstFont = font;
            sys::ImFontAtlas_AddFont(shared.as_ptr_mut(), &config);
        }
        atlas.build_alpha8_texture();
        (0x20..=0xff)
            .chain(characters)
            .filter_map(|c| Some((c, glyph_pixels(atlas, font, c)?)))
            .collect()
    }

    fn assert_original_glyphs(
        ctx: &mut Context,
        fonts: &[*mut sys::ImFont],
        original: &[BTreeMap<u32, GlyphPixels>],
        added: &[char],
    ) {
        for (&font, original) in fonts.iter().zip(original) {
            for &c in original
                .keys()
                .filter(|&&c| c <= 0xff || added.contains(&char::from_u32(c).unwrap()))
            {
                assert_eq!(
                    glyph_pixels(ctx.fonts(), font, c).as_ref(),
                    original.get(&c),
                    "pixel/metric mismatch: font size {}, character {c:#x}",
                    unsafe { (*font).FontSize }
                );
            }
        }
    }

    #[test]
    fn font_link_order_is_preserved_without_loading_files() {
        let links: Vec<_> =
            "A.TTC,First,128,96\0A.TTC,First\0B.TTF,Second\0\0".encode_utf16().collect();
        assert_eq!(parse_links(&links), [
            FontLink { file: "A.TTC".into(), face: "First".into() },
            FontLink { file: "B.TTF".into(), face: "Second".into() },
        ]);
    }

    #[test]
    fn glyph_pool_never_allocates_outside_its_fixed_bounds() {
        let mut pool = Shelf::default();
        assert!(pool.allocate(POOL_WIDTH, 10).is_none());
        for _ in 0..10000 {
            if let Some((x, y)) = pool.allocate(20, 24) {
                assert!(x + 20 < POOL_WIDTH && y + 24 < POOL_HEIGHT);
            }
        }
        assert!(pool.allocate(20, 24).is_none());
    }

    #[test]
    fn rendering_matches_original_at_all_sizes_and_stays_lazy() {
        struct Renderer {
            loads: usize,
            replacements: usize,
            size: (u32, u32),
        }
        impl RenderContext for Renderer {
            fn load_texture(
                &mut self,
                _: &[u8],
                width: u32,
                height: u32,
            ) -> windows::core::Result<imgui::TextureId> {
                self.loads += 1;
                self.size = (width, height);
                Ok(imgui::TextureId::new(1))
            }

            fn replace_texture(
                &mut self,
                texture: imgui::TextureId,
                _: &[u8],
                width: u32,
                height: u32,
            ) -> windows::core::Result<()> {
                // ID 0 is hudhook's original font texture, which is smaller.
                assert_eq!(texture, imgui::TextureId::new(1));
                assert_eq!((width, height), self.size);
                self.replacements += 1;
                Ok(())
            }
        }
        let mut ctx = Context::create();
        let ids: Vec<_> = [11., 18., 24.]
            .into_iter()
            .map(|size| {
                ctx.fonts().add_font(&[imgui::FontSource::TtfData {
                    data: include_bytes!("../../lib/data/ComicMono.ttf"),
                    size_pixels: size,
                    config: Some(imgui::FontConfig {
                        glyph_ranges: imgui::FontGlyphRanges::from_slice(&[0x20, 0xffff, 0]),
                        ..Default::default()
                    }),
                }])
            })
            .collect();
        ctx.fonts().build_rgba32_texture();
        let fonts: Vec<_> = ids
            .iter()
            .map(|&id| ctx.fonts().get_font(id).unwrap() as *const imgui::Font as *mut sys::ImFont)
            .collect();
        let mut fallback = FontFallback {
            links: Some(parse_links(
                &"TAHOMA.TTF,Tahoma\0MSYH.TTC,Microsoft YaHei UI\0\0"
                    .encode_utf16()
                    .collect::<Vec<_>>(),
            )),
            ..Default::default()
        };
        let mut renderer = Renderer { loads: 0, replacements: 0, size: (0, 0) };
        let start = Instant::now();
        let at = |milliseconds| start + Duration::from_millis(milliseconds);
        let request = |text: &str| {
            REQUESTS.lock().extend(
                fonts.iter().flat_map(|&font| text.chars().map(move |c| (font as usize, c))),
            )
        };
        REQUESTS.lock().clear();
        request("A");
        fallback.update_at(&mut ctx, &mut renderer, at(0));
        assert!(fallback.pool.is_none());
        assert_eq!(renderer.loads, 0);
        assert!(fallback.sources.is_empty());

        // Allocating the pool changes the atlas layout: upload immediately.
        request("中格");
        fallback.update_at(&mut ctx, &mut renderer, at(0));
        assert_eq!(renderer.loads, 1);
        assert_eq!(ctx.fonts().tex_id, imgui::TextureId::new(1));
        assert!(renderer.size.0 <= 2048 && renderer.size.1 <= 2048);
        assert_eq!(fallback.sources.len(), 1);
        let source = fallback.sources[&1].as_ref().unwrap();
        let original: Vec<_> = [11., 18., 24.]
            .into_iter()
            .map(|size| original_glyphs(size, source.data(), source.index))
            .collect();
        assert_original_glyphs(&mut ctx, &fonts, &original, &['中', '格']);
        for &font in &fonts {
            assert!(unsafe { sys::ImFont_FindGlyphNoFallback(font, '文' as u32).is_null() });
        }

        // New glyphs in fresh pool space share one upload after requests
        // settle.
        let additional: Vec<_> = "传送宁姆格福王之卢恩暗月大剑中文".chars().collect();
        request(&additional.iter().collect::<String>());
        fallback.update_at(&mut ctx, &mut renderer, at(10));
        assert_original_glyphs(&mut ctx, &fonts, &original, &additional);
        fallback.update_at(&mut ctx, &mut renderer, at(100));
        assert_eq!((renderer.loads, renderer.replacements), (1, 0));
        fallback.update_at(&mut ctx, &mut renderer, at(210));
        assert_eq!((renderer.loads, renderer.replacements), (1, 1));
        fallback.update_at(&mut ctx, &mut renderer, at(1000));
        assert_eq!((renderer.loads, renderer.replacements), (1, 1));

        // A steady stream of new glyphs is still uploaded within the maximum
        // delay.
        for (index, character) in "一二三四五六七八九十百".chars().enumerate() {
            request(&character.to_string());
            fallback.update_at(&mut ctx, &mut renderer, at(2000 + 100 * index as u64));
            let expected = if index < 10 { 1 } else { 2 };
            assert_eq!((renderer.loads, renderer.replacements), (1, expected));
        }

        // Eviction rebuilds the atlas, which resets its texture ID. The pool
        // texture must be replaced and kept in use immediately.
        fallback.shelf.y = POOL_HEIGHT;
        request("新");
        fallback.update_at(&mut ctx, &mut renderer, at(4000));
        assert_eq!((renderer.loads, renderer.replacements), (1, 3));
        assert_eq!(ctx.fonts().tex_id, imgui::TextureId::new(1));
        assert_original_glyphs(&mut ctx, &fonts, &original, &['新']);
        for &font in &fonts {
            assert!(!fallback.unavailable.contains(&(font as usize, '新')));
        }
        fallback.reset();
        assert!(fallback.sources.is_empty());
    }
}
