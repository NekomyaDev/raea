use blitz_traits::node_id::NodeId;
use selectors::context::QuirksMode;
use std::sync::atomic::Ordering as Ao;
use std::{
    io::Cursor,
    sync::{Arc, atomic::AtomicUsize, mpsc::Sender},
};
use style::{
    font_face::{FontFaceSourceFormat, FontFaceSourceFormatKeyword, FontStyleRange, Source},
    media_queries::MediaList,
    servo_arc::Arc as ServoArc,
    shared_lock::SharedRwLock,
    shared_lock::{Locked, SharedRwLockReadGuard},
    stylesheets::{
        AllowImportRules, CssRule, DocumentStyleSheet, ImportRule, Origin, Stylesheet,
        StylesheetInDocument, StylesheetLoader as ServoStylesheetLoader, UrlExtraData,
        import_rule::{ImportLayer, ImportSheet, ImportSupportsCondition},
    },
    values::{CssUrl, SourceLocation},
};

use blitz_traits::net::{AbortSignal, Bytes, NetHandler, NetProvider, Request};
use blitz_traits::shell::ShellProvider;

use url::Url;

use crate::{document::DocumentEvent, util::ImageType};

pub(crate) fn stamped_request(url: Url, signal: Option<&AbortSignal>) -> Request {
    let mut req = Request::get(url);
    if let Some(sig) = signal {
        req = req.signal(sig.clone());
    }
    req
}

/// Carries `@font-face` descriptors from CSS parsing through to font
/// registration so `parley::Collection::register_fonts` can alias the bytes
/// under the `font-family` declared in CSS rather than whatever family name
/// the TTF's own `name` table reports.
///
/// All fields are `Option` because each descriptor is independently optional
/// at the CSS level. Missing fields fall back to the values parley reads
/// from the font's own metadata.
#[derive(Clone, Debug, Default)]
pub struct FontFaceOverrides {
    /// `font-family` descriptor (the alias the rest of the stylesheet uses).
    pub family_name: Option<String>,
    /// `font-weight` descriptor as a single CSS weight (100–900). Stylo
    /// parses this as a range; we record the lower bound, which equals the
    /// upper bound in the common single-value case.
    pub weight: Option<f32>,
    /// `font-style` descriptor mapped to fontique's `FontStyle`.
    pub style: Option<parley::fontique::FontStyle>,
}

#[derive(Clone, Debug)]
pub enum Resource {
    Image(ImageType, u32, u32, Arc<Vec<u8>>),
    #[cfg(feature = "svg")]
    Svg(ImageType, crate::node::SvgImageData),
    Css(DocumentStyleSheet),
    /// Stylesheet loaded for an `@import` rule, to be attached to the rule on the document thread
    ImportedCss(ServoArc<Locked<ImportRule>>, ServoArc<Stylesheet>),
    Font(Bytes, FontFaceOverrides),
    /// HTML fetched for an `<iframe>` element's `src`
    DocumentSrc(String),
    None,
}

pub(crate) struct ResourceHandler<T: Send + Sync + 'static> {
    doc_id: usize,
    request_id: usize,
    node_id: Option<NodeId>,
    tx: Sender<DocumentEvent>,
    shell_provider: Arc<dyn ShellProvider>,
    data: T,
}

impl<T: Send + Sync + 'static> ResourceHandler<T> {
    pub(crate) fn new(
        tx: Sender<DocumentEvent>,
        doc_id: usize,
        node_id: Option<NodeId>,
        shell_provider: Arc<dyn ShellProvider>,
        data: T,
    ) -> Self {
        static REQUEST_ID_COUNTER: AtomicUsize = AtomicUsize::new(0);
        Self {
            request_id: REQUEST_ID_COUNTER.fetch_add(1, Ao::Relaxed),
            doc_id,
            node_id,
            tx,
            shell_provider,
            data,
        }
    }

    pub(crate) fn boxed(
        tx: Sender<DocumentEvent>,
        doc_id: usize,
        node_id: Option<NodeId>,
        shell_provider: Arc<dyn ShellProvider>,
        data: T,
    ) -> Box<dyn NetHandler>
    where
        ResourceHandler<T>: NetHandler,
    {
        Box::new(Self::new(tx, doc_id, node_id, shell_provider, data)) as _
    }

    pub(crate) fn request_id(&self) -> usize {
        self.request_id
    }

    fn respond(&self, resolved_url: String, result: Result<Resource, String>) {
        let response = ResourceLoadResponse {
            request_id: self.request_id,
            node_id: self.node_id,
            resolved_url: Some(resolved_url),
            result,
        };
        let _ = self.tx.send(DocumentEvent::ResourceLoad(response));
        self.shell_provider.request_redraw();
    }
}

#[allow(unused)]
pub struct ResourceLoadResponse {
    pub request_id: usize,
    pub node_id: Option<NodeId>,
    pub resolved_url: Option<String>,
    pub result: Result<Resource, String>,
}

pub struct StylesheetHandler {
    pub source_url: Url,
    pub guard: SharedRwLock,
    pub net_provider: Arc<dyn NetProvider>,
    pub abort_signal: Option<AbortSignal>,
}

impl NetHandler for ResourceHandler<StylesheetHandler> {
    fn bytes(self: Box<Self>, resolved_url: String, bytes: Bytes) {
        let Ok(css) = std::str::from_utf8(&bytes) else {
            return self.respond(resolved_url, Err(String::from("Invalid UTF8")));
        };

        // NOTE(Nico): I don't *think* external stylesheets should have HTML entities escaped
        // let escaped_css = html_escape::decode_html_entities(css);

        let sheet = Stylesheet::from_str(
            css,
            self.data.source_url.clone().into(),
            Origin::Author,
            ServoArc::new(self.data.guard.wrap(MediaList::empty())),
            self.data.guard.clone(),
            Some(&StylesheetLoader {
                tx: self.tx.clone(),
                doc_id: self.doc_id,
                net_provider: self.data.net_provider.clone(),
                shell_provider: self.shell_provider.clone(),
                abort_signal: self.data.abort_signal.clone(),
                import_depth: 0,
            }),
            crate::not_applied::reporter(), // raea: (#34) what stylo drops is noted
            QuirksMode::NoQuirks,
            AllowImportRules::Yes,
        );

        self.respond(
            resolved_url,
            Ok(Resource::Css(DocumentStyleSheet(ServoArc::new(sheet)))),
        );
    }
}

/// Maximum depth of nested `@import` rules. Imports nested deeper than this
/// are refused, preventing unbounded recursion (e.g. a stylesheet whose
/// resolved `@import` URL yields another stylesheet importing in turn, with
/// the URL growing geometrically at each level).
const MAX_IMPORT_DEPTH: usize = 16;

#[derive(Clone)]
pub(crate) struct StylesheetLoader {
    pub(crate) tx: Sender<DocumentEvent>,
    pub(crate) doc_id: usize,
    pub(crate) net_provider: Arc<dyn NetProvider>,
    pub(crate) shell_provider: Arc<dyn ShellProvider>,
    pub(crate) abort_signal: Option<AbortSignal>,
    /// Depth of `@import` nesting for the stylesheet this loader is parsing
    /// (0 for a top-level stylesheet)
    pub(crate) import_depth: usize,
}
impl ServoStylesheetLoader for StylesheetLoader {
    fn request_stylesheet(
        &self,
        url: CssUrl,
        location: SourceLocation,
        lock: &SharedRwLock,
        media: ServoArc<Locked<MediaList>>,
        supports: Option<ImportSupportsCondition>,
        layer: ImportLayer,
    ) -> ServoArc<Locked<ImportRule>> {
        if self.import_depth >= MAX_IMPORT_DEPTH || !supports.as_ref().is_none_or(|s| s.enabled) {
            return ServoArc::new(lock.wrap(ImportRule {
                url,
                stylesheet: ImportSheet::new_refused(),
                supports,
                layer,
                source_location: location,
            }));
        }

        let import = ImportRule {
            url,
            stylesheet: ImportSheet::new_pending(),
            supports,
            layer,
            source_location: location,
        };

        let url = import.url.url().unwrap().clone();
        let import = ServoArc::new(lock.wrap(import));
        self.net_provider.fetch(
            self.doc_id,
            stamped_request(url.as_ref().clone(), self.abort_signal.as_ref()),
            ResourceHandler::boxed(
                self.tx.clone(),
                self.doc_id,
                None, // node_id
                self.shell_provider.clone(),
                NestedStylesheetHandler {
                    url: url.clone(),
                    loader: StylesheetLoader {
                        import_depth: self.import_depth + 1,
                        ..self.clone()
                    },
                    lock: lock.clone(),
                    media,
                    import_rule: import.clone(),
                    net_provider: self.net_provider.clone(),
                },
            ),
        );

        import
    }
}

struct NestedStylesheetHandler {
    loader: StylesheetLoader,
    lock: SharedRwLock,
    url: ServoArc<Url>,
    media: ServoArc<Locked<MediaList>>,
    import_rule: ServoArc<Locked<ImportRule>>,
    net_provider: Arc<dyn NetProvider>,
}

impl NetHandler for ResourceHandler<NestedStylesheetHandler> {
    fn bytes(self: Box<Self>, resolved_url: String, bytes: Bytes) {
        let Ok(css) = std::str::from_utf8(&bytes) else {
            return self.respond(resolved_url, Err(String::from("Invalid UTF8")));
        };

        // NOTE(Nico): I don't *think* external stylesheets should have HTML entities escaped
        // let escaped_css = html_escape::decode_html_entities(css);

        let sheet = ServoArc::new(Stylesheet::from_str(
            css,
            UrlExtraData(self.data.url.clone()),
            Origin::Author,
            self.data.media.clone(),
            self.data.lock.clone(),
            Some(&self.data.loader),
            crate::not_applied::reporter(), // raea: (#34) what stylo drops is noted
            QuirksMode::NoQuirks,
            AllowImportRules::Yes,
        ));

        // Fetch @font-face fonts
        fetch_font_face(
            self.tx.clone(),
            self.doc_id,
            self.node_id,
            &sheet,
            &self.data.net_provider,
            &self.shell_provider,
            &self.data.lock.read(),
            self.data.loader.abort_signal.as_ref(),
        );

        let import_rule = self.data.import_rule.clone();
        self.respond(resolved_url, Ok(Resource::ImportedCss(import_rule, sheet)))
    }
}

struct FontFaceHandler {
    format: FontFaceSourceFormatKeyword,
    overrides: FontFaceOverrides,
}
impl NetHandler for ResourceHandler<FontFaceHandler> {
    fn bytes(mut self: Box<Self>, resolved_url: String, bytes: Bytes) {
        let result = self.data.parse(bytes);
        self.respond(resolved_url, result)
    }
}
impl FontFaceHandler {
    fn parse(&mut self, bytes: Bytes) -> Result<Resource, String> {
        if self.format == FontFaceSourceFormatKeyword::None && bytes.len() >= 4 {
            self.format = match &bytes.as_ref()[0..4] {
                // WOFF (v1) files begin with 0x774F4646 ('wOFF' in ascii)
                // See: <https://w3c.github.io/woff/woff1/spec/Overview.html#WOFFHeader>
                #[cfg(feature = "woff")]
                b"wOFF" => FontFaceSourceFormatKeyword::Woff,
                // WOFF2 files begin with 0x774F4632 ('wOF2' in ascii)
                // See: <https://w3c.github.io/woff/woff2/#woff20Header>
                #[cfg(feature = "woff")]
                b"wOF2" => FontFaceSourceFormatKeyword::Woff2,
                // Opentype fonts with CFF data begin with 0x4F54544F ('OTTO' in ascii)
                // See: <https://learn.microsoft.com/en-us/typography/opentype/spec/otff#organization-of-an-opentype-font>
                b"OTTO" => FontFaceSourceFormatKeyword::Opentype,
                // Opentype fonts truetype outlines begin with 0x00010000
                // See: <https://learn.microsoft.com/en-us/typography/opentype/spec/otff#organization-of-an-opentype-font>
                &[0x00, 0x01, 0x00, 0x00] => FontFaceSourceFormatKeyword::Truetype,
                // Truetype fonts begin with 0x74727565 ('true' in ascii)
                // See: <https://developer.apple.com/fonts/TrueType-Reference-Manual/RM06/Chap6.html#ScalerTypeNote>
                b"true" => FontFaceSourceFormatKeyword::Truetype,
                _ => FontFaceSourceFormatKeyword::None,
            }
        }

        // Satisfy rustc's mutability linting with woff feature both enabled/disabled
        #[cfg(feature = "woff")]
        let mut bytes = bytes;

        match self.format {
            #[cfg(feature = "woff")]
            FontFaceSourceFormatKeyword::Woff => {
                #[cfg(feature = "tracing")]
                tracing::info!("Decompressing woff1 font");

                // Use wuff crate to decompress font
                let decompressed = wuff::decompress_woff1(&bytes).ok();

                if let Some(decompressed) = decompressed {
                    bytes = Bytes::from(decompressed);
                } else {
                    #[cfg(feature = "tracing")]
                    tracing::warn!("Failed to decompress woff1 font");
                }
            }
            #[cfg(feature = "woff")]
            FontFaceSourceFormatKeyword::Woff2 => {
                #[cfg(feature = "tracing")]
                tracing::info!("Decompressing woff2 font");

                // Use wuff crate to decompress font
                let decompressed = wuff::decompress_woff2(&bytes).ok();

                if let Some(decompressed) = decompressed {
                    bytes = Bytes::from(decompressed);
                } else {
                    #[cfg(feature = "tracing")]
                    tracing::warn!("Failed to decompress woff2 font");
                }
            }
            FontFaceSourceFormatKeyword::None => {
                // Should this be an error?
                return Ok(Resource::None);
            }
            _ => {}
        }

        Ok(Resource::Font(bytes, std::mem::take(&mut self.overrides)))
    }
}

/// Fetch the fonts of all `@font-face` rules in `sheet`
#[allow(clippy::too_many_arguments)]
pub(crate) fn fetch_font_face(
    tx: Sender<DocumentEvent>,
    doc_id: usize,
    node_id: Option<NodeId>,
    sheet: &Stylesheet,
    network_provider: &Arc<dyn NetProvider>,
    shell_provider: &Arc<dyn ShellProvider>,
    read_guard: &SharedRwLockReadGuard,
    abort_signal: Option<&AbortSignal>,
) {
    fetch_font_face_rules(
        sheet.contents(read_guard).rules(read_guard).iter(),
        tx,
        doc_id,
        node_id,
        network_provider,
        shell_provider,
        read_guard,
        abort_signal,
    )
}

/// raea: the @font-face fetches not yet answered, as (document id, request id)
pub(crate) static FONT_LOADS: std::sync::Mutex<std::collections::BTreeSet<(usize, usize)>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Fetch the fonts of the `@font-face` rules among `rules`
#[allow(clippy::too_many_arguments)]
pub(crate) fn fetch_font_face_rules<'a>(
    rules: impl Iterator<Item = &'a CssRule>,
    tx: Sender<DocumentEvent>,
    doc_id: usize,
    node_id: Option<NodeId>,
    network_provider: &Arc<dyn NetProvider>,
    shell_provider: &Arc<dyn ShellProvider>,
    read_guard: &SharedRwLockReadGuard,
    abort_signal: Option<&AbortSignal>,
) {
    rules
        .filter_map(|rule| match rule {
            CssRule::FontFace(font_face) => {
                let descriptor = &font_face.read_with(read_guard).descriptors;
                let family = descriptor.font_family.as_ref()?;
                let src = descriptor.src.as_ref()?;
                // Capture the @font-face descriptors so parley can register
                // the font under the CSS-declared family name (and weight /
                // style) rather than whatever metadata the TTF reports.
                let overrides = FontFaceOverrides {
                    family_name: Some(family.name.to_string()),
                    weight: descriptor
                        .font_weight
                        .as_ref()
                        .and_then(|range| range.0.compute().map(|w| w.value())),
                    style: descriptor.font_style.as_ref().map(stylo_to_fontique_style),
                };
                Some((src, overrides))
            }
            _ => None,
        })
        .for_each(|(source_list, overrides)| {
            // Find the first font source in the source list that specifies a font of a type
            // that we support.
            let preferred_source = source_list
                .0
                .iter()
                .filter_map(|source| match source {
                    Source::Url(url_source) => Some(url_source),
                    // TODO: support local fonts in @font-face
                    Source::Local(_) => None,
                })
                .find_map(|url_source| {
                    let mut format = match &url_source.format_hint {
                        Some(FontFaceSourceFormat::Keyword(fmt)) => *fmt,
                        Some(FontFaceSourceFormat::String(str)) => match str.as_str() {
                            "woff2" => FontFaceSourceFormatKeyword::Woff2,
                            "ttf" => FontFaceSourceFormatKeyword::Truetype,
                            "otf" => FontFaceSourceFormatKeyword::Opentype,
                            _ => FontFaceSourceFormatKeyword::None,
                        },
                        _ => FontFaceSourceFormatKeyword::None,
                    };
                    // A url with no extension, such as a `data:` url, is
                    // sniffed from its bytes once fetched.
                    if format == FontFaceSourceFormatKeyword::None
                        && let Some((_, end)) = url_source.url.as_str().rsplit_once('.')
                    {
                        format = match end {
                            "woff2" => FontFaceSourceFormatKeyword::Woff2,
                            "woff" => FontFaceSourceFormatKeyword::Woff,
                            "ttf" => FontFaceSourceFormatKeyword::Truetype,
                            "otf" => FontFaceSourceFormatKeyword::Opentype,
                            "svg" => FontFaceSourceFormatKeyword::Svg,
                            "eot" => FontFaceSourceFormatKeyword::EmbeddedOpentype,
                            _ => FontFaceSourceFormatKeyword::None,
                        }
                    }

                    if matches!(
                        format,
                        FontFaceSourceFormatKeyword::Svg
                            | FontFaceSourceFormatKeyword::EmbeddedOpentype
                    ) {
                        #[cfg(feature = "tracing")]
                        tracing::warn!("Skipping unsupported font of type {:?}", format);
                        return None;
                    }

                    #[cfg(not(feature = "woff"))]
                    if matches!(
                        format,
                        FontFaceSourceFormatKeyword::Woff | FontFaceSourceFormatKeyword::Woff2
                    ) {
                        #[cfg(feature = "tracing")]
                        tracing::warn!("Skipping unsupported font of type {:?}", format);
                        return None;
                    }

                    // A relative url with no base url to resolve against
                    // yields None; skip the source instead of panicking
                    let Some(url) = url_source.url.url() else {
                        #[cfg(feature = "tracing")]
                        tracing::warn!("Skipping @font-face source with unresolvable url");
                        return None;
                    };
                    let url = url.as_ref().clone();
                    Some((url, format))
                });

            if let Some((url, format)) = preferred_source {
                let handler = ResourceHandler::new(
                    tx.clone(),
                    doc_id,
                    node_id,
                    shell_provider.clone(),
                    FontFaceHandler { format, overrides },
                );
                // raea: a document's faces still loading (`BaseDocument::has_pending_fonts`, document.fonts)
                FONT_LOADS.lock().unwrap().insert((doc_id, handler.request_id()));
                network_provider.fetch(doc_id, stamped_request(url, abort_signal), Box::new(handler));
            }
        })
}

/// Translate stylo's `@font-face` `font-style` descriptor into the fontique
/// `FontStyle` enum used by parley. Stylo encodes Italic and Oblique-with-
/// angle distinctly; CSS's bare `normal` is parsed as `Oblique(0deg, 0deg)`
/// by stylo (see the `FontStyle::parse` impl in stylo's `font_face.rs`), so
/// that pattern is treated as `Normal` here.
fn stylo_to_fontique_style(style: &FontStyleRange) -> parley::fontique::FontStyle {
    use parley::fontique::FontStyle as Fq;
    match style {
        FontStyleRange::Italic => Fq::Italic,
        FontStyleRange::Oblique(min, max) => {
            let angle = min.degrees();
            // Stylo emits `Oblique(0deg, 0deg)` for the literal CSS `normal`
            // keyword. Map that back to `Normal` so parley's font matching
            // doesn't misclassify upright fonts.
            if angle.is_none_or(|a| a == 0.0) && max.degrees().is_none_or(|a| a == 0.0) {
                Fq::Normal
            } else {
                Fq::Oblique(angle)
            }
        }
    }
}

/// Handles HTML fetched for an `<iframe>` element's `src`
pub(crate) struct DocumentSrcHandler;

impl NetHandler for ResourceHandler<DocumentSrcHandler> {
    fn bytes(self: Box<Self>, resolved_url: String, bytes: Bytes) {
        let html = String::from_utf8_lossy(&bytes).into_owned();
        self.respond(resolved_url, Ok(Resource::DocumentSrc(html)));
    }
}

/// raea: an app's decoder, tried before the image crate: a url and its bytes to a width, height and RGBA8 (sRGB)
/// pixels. raea-page's decodes what Bevy does (a data URL's format by its MIME type, DDS, float formats encoded as a
/// browser shows them).
pub static IMAGE_DECODER: std::sync::OnceLock<fn(&str, &[u8]) -> Option<(u32, u32, Vec<u8>)>> = std::sync::OnceLock::new();

/// raea: the net provider an <iframe>'s document loads through, in place of its parent's: raea-page's is confined to
/// RAEA_PAGE_ROOT, so a frame's src and what it loads can't read a file outside the app's root
/// (docs/blitz-bevy.feature B5c, iframes 6).
pub static FRAME_NET: std::sync::OnceLock<Arc<dyn NetProvider>> = std::sync::OnceLock::new();

pub struct ImageHandler {
    kind: ImageType,
}
impl ImageHandler {
    pub fn new(kind: ImageType) -> Self {
        Self { kind }
    }
}

impl NetHandler for ResourceHandler<ImageHandler> {
    fn bytes(self: Box<Self>, resolved_url: String, bytes: Bytes) {
        let result = self.data.parse(bytes, &resolved_url);
        self.respond(resolved_url, result)
    }
}

/// raea: a picture asked for at the size it's drawn: `raea-w=N` and/or `raea-h=N` in its URL's query decode it scaled
/// down to fit N pixels wide or high, its shape kept, never scaled up (the server doesn't read the query). The feed's
/// cards ask theirs at the card's size (raea-feed src/feed/media.ts): a 2560x1440 still held 14.7 MB decoded on a card
/// drawn 1440 pixels wide at most, 4.7 MB fitted (raea-feed e2e/memory.spec.ts). Area-averaged (`thumbnail`), on
/// premultiplied pixels when any is see-through, so no edge takes the colour of the transparent pixels beside it.
///
/// And one larger than the GPU's largest texture on either side (`set_max_image_side`, the side of Vello's image
/// atlas) is scaled down to fit it, its shape kept: Vello can only draw an image that fits its atlas, and a full-page
/// site capture (1080x18000) drawn 880 wide was refused outright (raea#245). Chrome draws it; here it's drawn from a
/// copy at most the atlas's side tall, which a page never draws larger than its own size anyway.
pub fn fit(url: &str, w: u32, h: u32, rgba: Vec<u8>) -> (u32, u32, Vec<u8>) {
    fit_within(url, w, h, rgba, MAX_IMAGE_SIDE.load(std::sync::atomic::Ordering::Relaxed))
}

fn fit_within(url: &str, w: u32, h: u32, mut rgba: Vec<u8>, side: u32) -> (u32, u32, Vec<u8>) {
    let query = url.split('#').next().and_then(|u| u.split_once('?')).map_or("", |(_, q)| q);
    let want = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(k)?.strip_prefix('=')?.parse::<u32>().ok()).filter(|n| *n > 0);
    let side = side.max(1) as f64;
    let scale = [
        want("raea-w").map(|n| n as f64 / w.max(1) as f64),
        want("raea-h").map(|n| n as f64 / h.max(1) as f64),
        (w.max(h) as f64 > side).then(|| side / w.max(h) as f64),
    ]
    .into_iter()
    .flatten()
    .fold(1.0f64, f64::min);
    if scale >= 1.0 || w == 0 || h == 0 || rgba.len() != w as usize * h as usize * 4 {
        return (w, h, rgba);
    }
    let (nw, nh) = (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1));
    let see_through = rgba.chunks_exact(4).any(|p| p[3] != 255);
    if see_through {
        for p in rgba.chunks_exact_mut(4) {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
    let Some(img) = image::RgbaImage::from_raw(w, h, rgba) else { unreachable!("the length was checked") };
    let mut out = image::imageops::thumbnail(&img, nw, nh).into_raw();
    if see_through {
        for p in out.chunks_exact_mut(4) {
            let a = p[3] as u32;
            if a > 0 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
    }
    (nw, nh, out)
}

/// raea: the longest side a decoded image keeps (`fit`): Vello's image atlas's side (the GPU's largest texture, 16384 at most).
/// 16384 until the page's renderer says (systems/page draw.rs, from the device's `max_texture_dimension_2d`)
static MAX_IMAGE_SIDE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(16384);

/// raea: the longest side an image is decoded at, from here on (the device's largest texture)
pub fn set_max_image_side(px: u32) {
    if px > 0 {
        MAX_IMAGE_SIDE.store(px, std::sync::atomic::Ordering::Relaxed);
    }
}

impl ImageHandler {
    fn parse(&self, bytes: Bytes, url: &str) -> Result<Resource, String> {
        if let Some((width, height, rgba)) = IMAGE_DECODER.get().and_then(|d| d(url, &bytes)) {
            let (width, height, rgba) = fit(url, width, height, rgba);
            return Ok(Resource::Image(self.kind, width, height, Arc::new(rgba)));
        }
        let mut reader = image::ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .expect("IO errors impossible with Cursor");
        // raea: a format with no signature to sniff (TGA) by its file's extension
        if reader.format().is_none() {
            let path = url.split(['?', '#']).next().unwrap_or(url);
            if let Ok(f) = image::ImageFormat::from_path(path) {
                reader.set_format(f);
            }
        }
        let image_err = match reader.decode() {
            Ok(image) => {
                let (width, height, raw_rgba8_data) = fit(url, image.width(), image.height(), image.into_rgba8().into_raw());
                return Ok(Resource::Image(
                    self.kind,
                    width,
                    height,
                    Arc::new(raw_rgba8_data),
                ));
            }
            Err(e) => e.to_string(),
        };

        #[cfg(feature = "svg")]
        let svg_err = {
            use crate::util::parse_svg_image;
            match parse_svg_image(&bytes) {
                Ok(svg) => return Ok(Resource::Svg(self.kind, svg)),
                Err(e) => e.to_string(),
            }
        };
        #[cfg(not(feature = "svg"))]
        let svg_err = "svg feature disabled";

        Err(format!(
            "Could not parse image ({} bytes): image-crate error: {image_err}; svg fallback error: {svg_err}",
            bytes.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_fits_the_size_its_url_asks() {
        let px = |w: u32, h: u32| vec![255u8; (w * h * 4) as usize];
        // the width asked, the shape kept
        let (w, h, b) = fit("http://127.0.0.1:9/media/a.png?raea-w=100", 400, 200, px(400, 200));
        assert_eq!((w, h, b.len()), (100, 50, 100 * 50 * 4));
        // the tighter of a width and a height; a fragment after the query is no part of it
        assert_eq!(fit("/media/a.png?x=1&raea-w=300&raea-h=50#t", 400, 200, px(400, 200)).0, 100);
        // never larger, and nothing asked is the picture as it is
        assert_eq!(fit("/media/a.png?raea-w=800", 400, 200, px(400, 200)).0, 400);
        assert_eq!(fit("/media/a.png", 400, 200, px(400, 200)).0, 400);
        assert_eq!(fit("/media/a.png?raea-wide=100", 400, 200, px(400, 200)).0, 400);
        // a see-through pixel beside an opaque red one keeps no colour of its own in the average
        let mut two = vec![255, 0, 0, 255, 0, 255, 0, 0];
        two.extend_from_slice(&[255, 0, 0, 255, 0, 255, 0, 0]);
        let (_, _, one) = fit("/a.png?raea-w=1", 2, 2, two);
        assert_eq!(one.len(), 4);
        assert!(one[1] < 8 && one[0] > 240, "green bled into red: {one:?}");
    }
    use parley::fontique::FontStyle as Fq;
    use style::values::specified::Angle;

    fn oblique(min_deg: f32, max_deg: f32) -> FontStyleRange {
        FontStyleRange::Oblique(Angle::from_degrees(min_deg), Angle::from_degrees(max_deg))
    }

    #[test]
    fn italic_maps_to_italic() {
        assert_eq!(stylo_to_fontique_style(&FontStyleRange::Italic), Fq::Italic,);
    }

    #[test]
    fn oblique_zero_zero_maps_to_normal() {
        // Stylo parses bare CSS `normal` as `Oblique(0deg, 0deg)`; the
        // helper must round-trip that back to `FontStyle::Normal` so
        // parley's matching doesn't misclassify upright fonts.
        assert_eq!(stylo_to_fontique_style(&oblique(0.0, 0.0)), Fq::Normal);
    }

    #[test]
    fn oblique_single_angle_maps_to_oblique_with_min() {
        assert_eq!(
            stylo_to_fontique_style(&oblique(14.0, 14.0)),
            Fq::Oblique(Some(14.0)),
        );
    }

    #[test]
    fn oblique_range_uses_min_angle() {
        // For a range, fontique's single-angle representation takes the
        // lower bound — confirm `min` (not `max`) is what gets through.
        assert_eq!(
            stylo_to_fontique_style(&oblique(10.0, 20.0)),
            Fq::Oblique(Some(10.0)),
        );
    }
}

#[cfg(test)]
mod fit_tests {
    use super::fit_within;

    // raea#245: a picture taller than the GPU's largest texture is decoded at a size that fits it, its shape kept
    #[test]
    fn an_image_past_the_atlas_side_is_fitted_to_it() {
        let (w, h) = (54, 900);
        let (nw, nh, px) = fit_within("tall.png", w, h, vec![200; (w * h * 4) as usize], 819);
        assert_eq!((nw, nh), (49, 819));
        assert_eq!(px.len(), (nw * nh * 4) as usize);
        assert!(px.iter().all(|&c| c == 200));
    }

    #[test]
    fn an_image_inside_it_is_left_alone_and_a_url_s_size_still_wins() {
        let (w, h) = (54, 800);
        assert_eq!(fit_within("a.png", w, h, vec![0; (w * h * 4) as usize], 819).0, 54);
        assert_eq!(fit_within("a.png?raea-h=400", w, h, vec![0; (w * h * 4) as usize], 819).1, 400);
    }
}
