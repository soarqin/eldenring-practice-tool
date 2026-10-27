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
use imgui::{sys, Context, Ui};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Registry::*;

mod packing;

use packing::RectPacker;

static REQUESTS: Lazy<Mutex<BTreeSet<(usize, char)>>> = Lazy::new(Mutex::default);
static UNAVAILABLE: Lazy<Mutex<BTreeSet<(usize, char)>>> = Lazy::new(Mutex::default);

// Bound retained glyph pixels without reserving space before it is needed.
const MAX_ATLAS_SIDE: u32 = 4096;
// hudhook 0.9.3 has eight descriptors and cannot safely reclaim/grow the
// heap through its public API. Icons and the original atlas already use two.
const MAX_TEXTURE_GROWTHS: usize = 6;
const UPLOAD_SETTLE: Duration = Duration::from_millis(200);
const UPLOAD_MAX_DELAY: Duration = Duration::from_secs(1);
const UPLOAD_RETRY: Duration = Duration::from_millis(500);

pub(crate) fn text_ready(ui: &Ui, text: &str) -> bool {
    let font = ui.current_font() as *const imgui::Font as *mut sys::ImFont;
    let unavailable = UNAVAILABLE.lock();
    text.chars().all(|c| {
        c.is_control()
            || unsafe { !sys::ImFont_FindGlyphNoFallback(font, c as u32).is_null() }
            || unavailable.contains(&(font as usize, c))
    })
}

pub(crate) fn request_text(ui: &Ui, text: &str) {
    let font = ui.current_font() as *const imgui::Font as *mut sys::ImFont;
    let unavailable = UNAVAILABLE.lock();
    let missing = text.chars().filter(|&c| {
        !c.is_control()
            && !unavailable.contains(&(font as usize, c))
            && unsafe { sys::ImFont_FindGlyphNoFallback(font, c as u32).is_null() }
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

struct PendingGlyph {
    metrics: sys::ImFontGlyph,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

struct Candidate {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    packing: RectPacker,
    glyphs: BTreeMap<(usize, char), PendingGlyph>,
    first: Instant,
    last: Instant,
}

impl Candidate {
    fn new(atlas: &mut imgui::FontAtlas, packing: Option<&RectPacker>, now: Instant) -> Self {
        let texture = atlas.build_rgba32_texture();
        let (width, height, pixels) = (texture.width, texture.height, texture.data.to_vec());
        let packing = packing.cloned().unwrap_or_else(|| unsafe {
            let raw = atlas.raw();
            let mut packing = RectPacker::new(width, height);
            let mut reserve = |x0: u32, y0: u32, x1: u32, y1: u32| {
                let padding = raw.TexGlyphPadding.max(1) as u32;
                let (x, y) = (x0.saturating_sub(padding), y0.saturating_sub(padding));
                let right = x1.saturating_add(padding).min(width);
                let bottom = y1.saturating_add(padding).min(height);
                if right > x && bottom > y {
                    packing.reserve(x, y, right - x, bottom - y);
                }
            };
            for &font in std::slice::from_raw_parts(raw.Fonts.Data, raw.Fonts.Size as usize) {
                for glyph in
                    std::slice::from_raw_parts((*font).Glyphs.Data, (*font).Glyphs.Size as usize)
                {
                    if glyph.U1 > glyph.U0 && glyph.V1 > glyph.V0 {
                        reserve(
                            (glyph.U0 * width as f32).round() as u32,
                            (glyph.V0 * height as f32).round() as u32,
                            (glyph.U1 * width as f32).round() as u32,
                            (glyph.V1 * height as f32).round() as u32,
                        );
                    }
                }
            }
            // Custom rectangles also hold ImGui's white pixel, lines and
            // cursors.
            for rect in
                std::slice::from_raw_parts(raw.CustomRects.Data, raw.CustomRects.Size as usize)
            {
                if rect.X != u16::MAX && rect.Y != u16::MAX {
                    reserve(
                        u32::from(rect.X),
                        u32::from(rect.Y),
                        u32::from(rect.X) + u32::from(rect.Width),
                        u32::from(rect.Y) + u32::from(rect.Height),
                    );
                }
            }
            packing
        });
        Self { width, height, pixels, packing, glyphs: BTreeMap::new(), first: now, last: now }
    }

    fn allocate(&mut self, width: u32, height: u32, allow_growth: bool) -> Option<(u32, u32)> {
        let (width, height) = (width.max(1).checked_add(2)?, height.max(1).checked_add(2)?);
        if width > MAX_ATLAS_SIDE || height > MAX_ATLAS_SIDE {
            return None;
        }
        loop {
            if let Some((x, y)) = self.packing.allocate(width, height) {
                return Some((x + 1, y + 1));
            }
            if !allow_growth {
                return None;
            }
            let next_width = (self.width * 2).min(MAX_ATLAS_SIDE);
            let next_height = (self.height * 2).min(MAX_ATLAS_SIDE);
            if next_width <= self.width && next_height <= self.height {
                return None;
            }
            // Transparent padding must stay white for ImGui's linear sampler.
            let mut pixels = vec![255; next_width as usize * next_height as usize * 4];
            for alpha in pixels.iter_mut().skip(3).step_by(4) {
                *alpha = 0;
            }
            let old_stride = self.width as usize * 4;
            let new_stride = next_width as usize * 4;
            for row in 0..self.height as usize {
                pixels[row * new_stride..row * new_stride + old_stride]
                    .copy_from_slice(&self.pixels[row * old_stride..(row + 1) * old_stride]);
            }
            self.packing.grow(next_width, next_height);
            self.pixels = pixels;
            self.width = next_width;
            self.height = next_height;
        }
    }

    fn insert(&mut self, font: usize, glyph: Glyph, now: Instant, allow_growth: bool) -> bool {
        let Some((x, y)) = self.allocate(glyph.width, glyph.height, allow_growth) else {
            return false;
        };
        for row in 0..glyph.height {
            for column in 0..glyph.width {
                let alpha = glyph.bitmap[(row * glyph.width + column) as usize];
                let offset = (((y + row) * self.width + x + column) * 4) as usize;
                self.pixels[offset..offset + 4].copy_from_slice(&[255, 255, 255, alpha]);
            }
        }
        self.glyphs.insert((font, glyph.character), PendingGlyph {
            metrics: glyph.metrics,
            x,
            y,
            width: glyph.width,
            height: glyph.height,
        });
        self.last = now;
        true
    }

    fn commit(self, atlas: &mut imgui::FontAtlas, pixels: *mut u32) -> RectPacker {
        unsafe {
            let raw = atlas.raw_mut();
            if (raw.TexWidth, raw.TexHeight) != (self.width as i32, self.height as i32) {
                let scale_x = raw.TexWidth as f32 / self.width as f32;
                let scale_y = raw.TexHeight as f32 / self.height as f32;
                for &font in std::slice::from_raw_parts(raw.Fonts.Data, raw.Fonts.Size as usize) {
                    for glyph in std::slice::from_raw_parts_mut(
                        (*font).Glyphs.Data,
                        (*font).Glyphs.Size as usize,
                    ) {
                        glyph.U0 *= scale_x;
                        glyph.U1 *= scale_x;
                        glyph.V0 *= scale_y;
                        glyph.V1 *= scale_y;
                    }
                }
                raw.TexUvWhitePixel.x *= scale_x;
                raw.TexUvWhitePixel.y *= scale_y;
                for uv in &mut raw.TexUvLines {
                    uv.x *= scale_x;
                    uv.z *= scale_x;
                    uv.y *= scale_y;
                    uv.w *= scale_y;
                }
            }
            sys::ImFontAtlas_ClearTexData(raw);
            raw.TexPixelsRGBA32 = pixels;
            raw.TexWidth = self.width as i32;
            raw.TexHeight = self.height as i32;
            raw.TexUvScale = sys::ImVec2 { x: 1. / self.width as f32, y: 1. / self.height as f32 };
            raw.TexReady = true;

            let mut changed = BTreeSet::new();
            for ((font, character), glyph) in self.glyphs {
                sys::ImFont_AddGlyph(
                    font as *mut sys::ImFont,
                    ptr::null(),
                    character as u32,
                    glyph.metrics.X0,
                    glyph.metrics.Y0,
                    glyph.metrics.X1,
                    glyph.metrics.Y1,
                    glyph.x as f32 / self.width as f32,
                    glyph.y as f32 / self.height as f32,
                    (glyph.x + glyph.width) as f32 / self.width as f32,
                    (glyph.y + glyph.height) as f32 / self.height as f32,
                    glyph.metrics.AdvanceX,
                );
                changed.insert(font);
            }
            for font in changed {
                sys::ImFont_BuildLookupTable(font as *mut sys::ImFont);
            }
        }
        self.packing
    }
}

#[derive(Default)]
pub(crate) struct FontFallback {
    links: Option<Vec<FontLink>>,
    sources: BTreeMap<usize, Option<MappedFont>>,
    packing: Option<RectPacker>,
    candidate: Option<Candidate>,
    retry_at: Option<Instant>,
    texture_growths: usize,
}

impl FontFallback {
    pub(crate) fn reset(&mut self) {
        REQUESTS.lock().clear();
        UNAVAILABLE.lock().clear();
        *self = Self::default();
    }

    pub(crate) fn update(&mut self, ctx: &mut Context, renderer: &mut dyn RenderContext) {
        self.update_at(ctx, renderer, Instant::now());
    }

    fn update_at(&mut self, ctx: &mut Context, renderer: &mut dyn RenderContext, now: Instant) {
        let requested = std::mem::take(&mut *REQUESTS.lock());
        if requested.is_empty() && self.candidate.is_none() {
            return;
        }
        let mut faces = BTreeMap::<usize, Option<Face>>::new();
        let mut batches = BTreeMap::<(usize, usize), Vec<char>>::new();
        for (font, character) in requested {
            if UNAVAILABLE.lock().contains(&(font, character))
                || self
                    .candidate
                    .as_ref()
                    .is_some_and(|candidate| candidate.glyphs.contains_key(&(font, character)))
                || unsafe {
                    !sys::ImFont_FindGlyphNoFallback(font as *mut sys::ImFont, character as u32)
                        .is_null()
                }
            {
                continue;
            }
            let links = self.links.get_or_insert_with(font_links);
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
                UNAVAILABLE.lock().insert((font, character));
                continue;
            };
            batches.entry((font, source)).or_default().push(character);
        }
        for ((font, source), characters) in batches {
            let mut glyphs = self.sources[&source]
                .as_ref()
                .unwrap()
                .rasterize(font as *mut sys::ImFont, &characters)
                .into_iter()
                .peekable();
            // Rasterization preserves request order, including when glyphs
            // fail.
            for character in characters {
                let Some(glyph) = glyphs.next_if(|glyph| glyph.character == character) else {
                    UNAVAILABLE.lock().insert((font, character));
                    continue;
                };
                let candidate = self
                    .candidate
                    .get_or_insert_with(|| Candidate::new(ctx.fonts(), self.packing.as_ref(), now));
                if !candidate.insert(font, glyph, now, self.texture_growths < MAX_TEXTURE_GROWTHS) {
                    UNAVAILABLE.lock().insert((font, character));
                    warn!("Font atlas capacity limit reached; using the fallback character");
                }
            }
        }

        let Some(candidate) = self.candidate.as_ref() else {
            return;
        };
        if candidate.glyphs.is_empty() {
            self.candidate = None;
            return;
        }
        if self.retry_at.is_some_and(|retry| now < retry)
            || (now.duration_since(candidate.last) < UPLOAD_SETTLE
                && now.duration_since(candidate.first) < UPLOAD_MAX_DELAY)
        {
            return;
        }

        let resized = unsafe {
            let atlas = ctx.fonts().raw();
            (atlas.TexWidth, atlas.TexHeight) != (candidate.width as i32, candidate.height as i32)
        };
        if resized && self.texture_growths == MAX_TEXTURE_GROWTHS {
            UNAVAILABLE.lock().extend(candidate.glyphs.keys().copied());
            self.candidate = None;
            self.retry_at = None;
            warn!("Font atlas texture limit reached; using the fallback character");
            return;
        }

        // Reserve ImGui-owned pixels before changing the GPU texture. A failed
        // upload must leave all live glyphs, UVs and CPU pixels intact.
        let pixels = unsafe { sys::igMemAlloc(candidate.pixels.len()).cast::<u32>() };
        if pixels.is_null() {
            self.retry_at = Some(now + UPLOAD_RETRY);
            warn!("Could not allocate font atlas pixels");
            return;
        }
        unsafe {
            ptr::copy_nonoverlapping(
                candidate.pixels.as_ptr(),
                pixels.cast(),
                candidate.pixels.len(),
            );
        }
        let texture = ctx.fonts().tex_id;
        let uploaded = if resized {
            // Keep one active atlas. Old textures remain owned by hudhook;
            // count failed loads too, since they may already own a descriptor.
            self.texture_growths += 1;
            renderer.load_texture(&candidate.pixels, candidate.width, candidate.height)
        } else {
            renderer
                .replace_texture(texture, &candidate.pixels, candidate.width, candidate.height)
                .map(|()| texture)
        };
        match uploaded {
            Ok(texture) => {
                self.packing = Some(self.candidate.take().unwrap().commit(ctx.fonts(), pixels));
                ctx.fonts().tex_id = texture;
                self.retry_at = None;
            },
            Err(error) => {
                unsafe { sys::igMemFree(pixels.cast()) };
                self.retry_at = Some(now + UPLOAD_RETRY);
                warn!("Could not upload fallback glyphs: {error}");
            },
        }
    }
}

#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    fn glyphs_publish_only_after_upload_and_survive_growth() {
        let _lock = IMGUI_TEST_LOCK.lock().unwrap();
        struct Renderer {
            attempts: usize,
            uploads: usize,
            loads: usize,
            texture: imgui::TextureId,
            size: (u32, u32),
            pixels: Vec<u8>,
            fail: bool,
            unpublished: Vec<(usize, char)>,
        }
        impl Renderer {
            fn upload(
                &mut self,
                pixels: &[u8],
                width: u32,
                height: u32,
            ) -> windows::core::Result<()> {
                for &(font, character) in &self.unpublished {
                    assert!(unsafe {
                        sys::ImFont_FindGlyphNoFallback(font as *mut sys::ImFont, character as u32)
                            .is_null()
                    });
                }
                self.attempts += 1;
                if self.fail {
                    return Err(windows::core::Error::from_hresult(windows::core::HRESULT(-1)));
                }
                self.size = (width, height);
                self.pixels = pixels.to_vec();
                self.uploads += 1;
                Ok(())
            }
        }
        impl RenderContext for Renderer {
            fn load_texture(
                &mut self,
                pixels: &[u8],
                width: u32,
                height: u32,
            ) -> windows::core::Result<imgui::TextureId> {
                self.loads += 1;
                self.upload(pixels, width, height)?;
                self.texture = imgui::TextureId::new(7 + self.loads);
                Ok(self.texture)
            }

            fn replace_texture(
                &mut self,
                texture: imgui::TextureId,
                pixels: &[u8],
                width: u32,
                height: u32,
            ) -> windows::core::Result<()> {
                assert_eq!(texture, self.texture);
                // The unmodified DX12 backend rejects dimension changes.
                assert_eq!((width, height), self.size);
                self.upload(pixels, width, height)
            }
        }

        let mut ctx = Context::create();
        ctx.set_ini_filename(None);
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
        let texture = ctx.fonts().build_rgba32_texture();
        let initial_size = (texture.width, texture.height);
        let initial_pixels = texture.data.to_vec();
        ctx.fonts().tex_id = imgui::TextureId::new(7);
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
        let mut renderer = Renderer {
            attempts: 0,
            uploads: 0,
            loads: 0,
            texture: imgui::TextureId::new(7),
            size: initial_size,
            pixels: initial_pixels.clone(),
            fail: false,
            unpublished: Vec::new(),
        };
        let start = Instant::now();
        let at = |milliseconds| start + Duration::from_millis(milliseconds);
        let request = |text: &str| {
            REQUESTS.lock().extend(
                fonts.iter().flat_map(|&font| text.chars().map(move |c| (font as usize, c))),
            )
        };
        let keys = |text: &str| -> Vec<_> {
            fonts.iter().flat_map(|&font| text.chars().map(move |c| (font as usize, c))).collect()
        };
        REQUESTS.lock().clear();
        UNAVAILABLE.lock().clear();

        request("A");
        fallback.update_at(&mut ctx, &mut renderer, at(0));
        assert!(fallback.candidate.is_none());
        assert!(fallback.packing.is_none());
        assert!(fallback.sources.is_empty());
        assert_eq!(renderer.attempts, 0);

        request("中格");
        fallback.update_at(&mut ctx, &mut renderer, at(0));
        renderer.unpublished = keys("中格");
        assert!(fallback.candidate.is_some());
        assert_eq!(ctx.fonts().build_rgba32_texture().data, initial_pixels);
        assert_eq!(renderer.uploads, 0);
        // Repeated visible requests must not rebuild glyphs or postpone upload.
        request("中格");
        fallback.update_at(&mut ctx, &mut renderer, at(100));
        fallback.update_at(&mut ctx, &mut renderer, at(200));
        assert_eq!(renderer.uploads, 1);
        renderer.unpublished.clear();
        assert_eq!(fallback.sources.len(), 1);
        let source = fallback.sources[&1].as_ref().unwrap();
        let original: Vec<_> = [11., 18., 24.]
            .into_iter()
            .map(|size| original_glyphs(size, source.data(), source.index))
            .collect();
        assert_original_glyphs(&mut ctx, &fonts, &original, &['中', '格']);

        let additional = "传送宁姆格福王之卢恩暗月大剑中文";
        let committed_size = renderer.size;
        let committed_pixels = renderer.pixels.clone();
        request(additional);
        fallback.update_at(&mut ctx, &mut renderer, at(210));
        renderer.unpublished = keys("传送文");
        renderer.fail = true;
        fallback.update_at(&mut ctx, &mut renderer, at(410));
        assert_eq!((renderer.attempts, renderer.uploads), (2, 1));
        assert_eq!(renderer.size, committed_size);
        assert_eq!(ctx.fonts().build_rgba32_texture().data, committed_pixels);
        assert_original_glyphs(&mut ctx, &fonts, &original, &['中', '格']);
        request(additional);
        fallback.update_at(&mut ctx, &mut renderer, at(600));
        assert_eq!(renderer.attempts, 2);
        renderer.fail = false;
        fallback.update_at(&mut ctx, &mut renderer, at(910));
        assert_eq!((renderer.attempts, renderer.uploads), (3, 2));
        renderer.unpublished.clear();
        assert_original_glyphs(
            &mut ctx,
            &fonts,
            &original,
            &additional.chars().collect::<Vec<_>>(),
        );

        let before_size = renderer.size;
        let before_texture = ctx.fonts().tex_id;
        let before_growths = fallback.texture_growths;
        let before_loads = renderer.loads;
        let before_white = unsafe { ctx.fonts().raw().TexUvWhitePixel };
        let before_lines = unsafe { ctx.fonts().raw().TexUvLines };
        // Occupy the current free space to force the next glyph across a
        // texture-size boundary without rasterizing thousands of characters.
        assert!(fallback.packing.as_mut().unwrap().reserve(0, 0, before_size.0, before_size.1));
        request("新");
        fallback.update_at(&mut ctx, &mut renderer, at(1000));
        let candidate = fallback.candidate.as_ref().unwrap();
        assert_ne!((candidate.width, candidate.height), before_size);
        assert_eq!(renderer.size, before_size);
        assert_eq!(ctx.fonts().tex_id, before_texture);
        assert_eq!(ctx.fonts().build_rgba32_texture().data, renderer.pixels);
        let pending = &candidate.glyphs[&(fonts[0] as usize, '新')];
        let padding = (((pending.y - 1) * candidate.width + pending.x - 1) * 4) as usize;
        assert_eq!(&candidate.pixels[padding..padding + 4], &[255, 255, 255, 0]);
        renderer.unpublished = keys("新");
        renderer.fail = true;
        fallback.update_at(&mut ctx, &mut renderer, at(1200));
        assert_eq!(renderer.uploads, 2);
        assert_eq!(renderer.loads, before_loads + 1);
        assert_eq!(fallback.texture_growths, before_growths + 1);
        assert_eq!(ctx.fonts().tex_id, before_texture);
        assert_eq!(ctx.fonts().build_rgba32_texture().data, renderer.pixels);
        fallback.update_at(&mut ctx, &mut renderer, at(1500));
        assert_eq!(renderer.loads, before_loads + 1);
        renderer.fail = false;
        fallback.update_at(&mut ctx, &mut renderer, at(1700));
        renderer.unpublished.clear();
        assert_eq!(renderer.uploads, 3);
        assert_eq!(renderer.loads, before_loads + 2);
        assert_eq!(fallback.texture_growths, before_growths + 2);
        assert_ne!(ctx.fonts().tex_id, before_texture);
        assert_eq!(ctx.fonts().tex_id, renderer.texture);
        assert_eq!(ctx.fonts().build_rgba32_texture().data, renderer.pixels);
        let after_white = unsafe { ctx.fonts().raw().TexUvWhitePixel };
        assert_eq!(before_white.x * before_size.0 as f32, after_white.x * renderer.size.0 as f32);
        assert_eq!(before_white.y * before_size.1 as f32, after_white.y * renderer.size.1 as f32);
        for (before, after) in before_lines.iter().zip(unsafe { ctx.fonts().raw().TexUvLines }) {
            assert_eq!(before.x * before_size.0 as f32, after.x * renderer.size.0 as f32);
            assert_eq!(before.y * before_size.1 as f32, after.y * renderer.size.1 as f32);
            assert_eq!(before.z * before_size.0 as f32, after.z * renderer.size.0 as f32);
            assert_eq!(before.w * before_size.1 as f32, after.w * renderer.size.1 as f32);
        }
        let mut all = additional.chars().collect::<Vec<_>>();
        all.push('新');
        assert_original_glyphs(&mut ctx, &fonts, &original, &all);

        // New requests arriving continuously still publish within one second.
        for (index, character) in "一二三四五六七八九十百".chars().enumerate() {
            request(&character.to_string());
            fallback.update_at(&mut ctx, &mut renderer, at(2000 + 100 * index as u64));
            assert_eq!(renderer.uploads, if index < 10 { 3 } else { 4 });
        }
        request("😀");
        fallback.update_at(&mut ctx, &mut renderer, at(4000));
        assert!(fonts.iter().all(|&font| UNAVAILABLE.lock().contains(&(font as usize, '😀'))));
        assert!(fallback.candidate.is_none());
        // Exhausting descriptors must still allow uploads into existing holes.
        let attempts = renderer.attempts;
        let loads = renderer.loads;
        let texture = ctx.fonts().tex_id;
        fallback.texture_growths = MAX_TEXTURE_GROWTHS;
        let glyph =
            fallback.sources[&1].as_ref().unwrap().rasterize(fonts[0], &['容']).pop().unwrap();
        let (width, height) = (glyph.width.max(1) + 2, glyph.height.max(1) + 2);
        let packing = fallback.packing.as_mut().unwrap();
        let (x, y) = packing.clone().allocate(width, height).unwrap();
        // Leave exactly one glyph-sized hole in otherwise occupied space.
        packing.reserve(0, 0, x, renderer.size.1);
        packing.reserve(x + width, 0, renderer.size.0 - x - width, renderer.size.1);
        packing.reserve(0, 0, renderer.size.0, y);
        packing.reserve(0, y + height, renderer.size.0, renderer.size.1 - y - height);
        REQUESTS.lock().extend([(fonts[0] as usize, '容'), (fonts[0] as usize, '量')]);
        fallback.update_at(&mut ctx, &mut renderer, at(4100));
        assert_eq!(fallback.candidate.as_ref().unwrap().glyphs.len(), 1);
        fallback.update_at(&mut ctx, &mut renderer, at(4300));
        assert!(fallback.candidate.is_none());
        assert_eq!(renderer.attempts, attempts + 1);
        assert_eq!(renderer.loads, loads);
        assert_eq!(ctx.fonts().tex_id, texture);
        assert!(glyph_pixels(ctx.fonts(), fonts[0], '容' as u32).is_some());
        assert!(!UNAVAILABLE.lock().contains(&(fonts[0] as usize, '容')));
        assert!(UNAVAILABLE.lock().contains(&(fonts[0] as usize, '量')));

        // The original API may retain a descriptor even after a failed load.
        // Without holes, settle to a visible fallback without another upload.
        assert!(fallback.packing.as_mut().unwrap().reserve(0, 0, renderer.size.0, renderer.size.1));
        request("限");
        fallback.update_at(&mut ctx, &mut renderer, at(4500));
        assert!(fallback.candidate.is_none());
        assert_eq!(renderer.attempts, attempts + 1);
        assert_eq!(ctx.fonts().tex_id, texture);
        assert_eq!(ctx.fonts().build_rgba32_texture().data, renderer.pixels);
        assert!(fonts.iter().all(|&font| UNAVAILABLE.lock().contains(&(font as usize, '限'))));
        fallback.reset();
        assert!(fallback.sources.is_empty());
        assert!(UNAVAILABLE.lock().is_empty());
    }
}
