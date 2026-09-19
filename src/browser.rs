// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! Browser application. JavaScript is only wasm-bindgen's generated platform
//! glue; file decoding, DOM events, state, conversion and exports live here.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::rc::Rc;

use wasm_bindgen::{closure::Closure, prelude::*, JsCast};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{
    Blob, BlobPropertyBag, CanvasRenderingContext2d, Document, Element, Event, File,
    HtmlAnchorElement, HtmlCanvasElement, HtmlElement, HtmlFieldSetElement, HtmlImageElement,
    HtmlInputElement, HtmlSelectElement, ImageBitmap, ImageData, KeyboardEvent, PointerEvent, Url,
    WheelEvent,
};

use crate::mosaic::{crop_frame, crop_frame_options, FitMode, HueMode, Mosaic, Options, Order, Raster};
use crate::{palette, render};

type Shared = Rc<RefCell<App>>;

/// Wire format of the worker's successful reply.
#[allow(clippy::type_complexity)]
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum StageImage {
    None,
    Some {
        width: usize,
        height: usize,
        pixels: Vec<u8>,
    },
}
type StagePayload = (StageImage, StageImage, StageImage, StageImage);
#[allow(clippy::type_complexity)] // worker wire format
type WorkerReply = (
    Vec<usize>,
    usize,
    usize,
    Vec<usize>,
    Vec<(usize, usize)>,
    StagePayload,
);

#[derive(Default)]
struct App {
    file: Option<File>,
    raster: Option<Raster>,
    mosaic: Option<Mosaic>,
    svg: String,
    guide: String,
    csv: String,
    source_url: Option<String>,
    mosaic_url: Option<String>,
    exclusions: BTreeMap<String, BTreeSet<usize>>,
    busy: bool,
    job: u64,
    worker: Option<web_sys::Worker>,
    worker_message: Option<Closure<dyn FnMut(web_sys::MessageEvent)>>,
    worker_error: Option<Closure<dyn FnMut(Event)>>,
    timeout: Option<i32>,
    timer_callback: Option<Closure<dyn FnMut()>>,
    elapsed: f64,
    /// Interactive crop: overlay geometry, in-flight pointer drag, and
    /// whether the frame mirrors the current "Fill frame" options.
    crop: CropUi,
}

/// Everything the interactive crop frame needs between events.
#[derive(Default)]
struct CropUi {
    /// Source-rect frame of the options currently drawn on screen.
    drawn: Option<CropFrame>,
    /// Pointer drag in flight: which handle, offset from its origin.
    drag: Option<CropDrag>,
}

#[derive(Clone, Copy)]
struct CropFrame {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[derive(Clone, Copy)]
struct CropDrag {
    /// Corner being resized, or None for a whole-frame pan.
    corner: Option<CropHandle>,
    /// Pointer offset from the grabbed corner / frame origin, source px.
    grab_x: f64,
    grab_y: f64,
    /// Frame at gesture start, for clamping and corner anchoring.
    origin: CropFrame,
}

fn document() -> Result<Document, JsValue> {
    web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| JsValue::from_str("Browser document unavailable"))
}

fn element(id: &str) -> Result<Element, JsValue> {
    document()?
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("Missing interface element: {id}")))
}

fn input(id: &str) -> Result<HtmlInputElement, JsValue> {
    Ok(element(id)?.dyn_into()?)
}
fn select(id: &str) -> Result<HtmlSelectElement, JsValue> {
    Ok(element(id)?.dyn_into()?)
}
fn value(id: &str) -> Result<String, JsValue> {
    Ok(input(id)?.value())
}
fn number(id: &str) -> Result<f64, JsValue> {
    value(id)?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| JsValue::from_str(&format!("Please enter a valid {id}.")))
}
fn text(id: &str, value: &str) -> Result<(), JsValue> {
    element(id)?.set_text_content(Some(value));
    Ok(())
}
fn hidden(id: &str, hide: bool) -> Result<(), JsValue> {
    let el = element(id)?;
    if hide {
        el.set_attribute("hidden", "")?;
    } else {
        el.remove_attribute("hidden")?;
    }
    Ok(())
}

fn show_error(error: JsValue) {
    let message = error
        .as_string()
        .or_else(|| {
            js_sys::Reflect::get(&error, &JsValue::from_str("message"))
                .ok()
                .and_then(|v| v.as_string())
        })
        .unwrap_or_else(|| {
            "The browser could not complete this action. Try a PNG or JPEG, or use a smaller image."
                .into()
        });
    let _ = text("error", &message);
    let _ = hidden("error", false);
}

fn listen(
    id: &str,
    event: &str,
    mut handler: impl FnMut(Event) -> Result<(), JsValue> + 'static,
) -> Result<(), JsValue> {
    let callback = Closure::<dyn FnMut(Event)>::new(move |event| {
        if let Err(error) = handler(event) {
            show_error(error);
        }
    });
    element(id)?.add_event_listener_with_callback(event, callback.as_ref().unchecked_ref())?;
    // These are a fixed number of document-lifetime handlers. Dynamic swatches
    // use one delegated handler; no closures accumulate when colors change.
    callback.forget();
    Ok(())
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn set_busy(state: &Shared, busy: bool, message: &str) -> Result<(), JsValue> {
    state.borrow_mut().busy = busy;
    element("settings")?
        .dyn_into::<HtmlFieldSetElement>()?
        .set_disabled(busy);
    element("form")?.set_class_name(if busy { "busy" } else { "" });
    text("go", if busy { "Working…" } else { "Build mosaic" })?;
    text("status", message)?;
    element("status")?.set_class_name("status");
    element("preview")?.set_attribute("aria-busy", if busy { "true" } else { "false" })?;
    update_labels_with(busy)
}

fn mark_pending(state: &Shared) -> Result<(), JsValue> {
    if !state.borrow().busy {
        text(
            "status",
            if state.borrow().raster.is_some() {
                "Settings changed. Build to update your mosaic and downloads."
            } else {
                "Choose an image, then build your mosaic."
            },
        )?;
        element("status")?.set_class_name("status pending");
        if state.borrow().mosaic.is_some() {
            text(
                "build-meta",
                "Settings changed — these outputs belong to your previous build.",
            )?;
        }
        if state.borrow().mosaic.is_some() {
            text("result-label", "Previous build · settings changed")?;
        }
    }
    Ok(())
}

fn populate_palettes() -> Result<(), JsValue> {
    let mut html = String::new();
    for (id, label, _) in palette::all() {
        let _ = write!(
            html,
            "<option value=\"{}\">{}</option>",
            escape(id),
            escape(label)
        );
    }
    element("palette")?.set_inner_html(&html);
    Ok(())
}

fn swatches(state: &Shared) -> Result<(), JsValue> {
    let id = select("palette")?.value();
    let tiles = palette::all()
        .iter()
        .find(|p| p.0 == id)
        .ok_or_else(|| JsValue::from_str("Unknown palette"))?
        .2;
    let app = state.borrow();
    let mut html = String::new();
    for (i, tile) in tiles.iter().enumerate() {
        let checked = if app.exclusions.get(&id).is_some_and(|set| set.contains(&i)) {
            ""
        } else {
            " checked"
        };
        let _ = write!(html, "<label class=\"swatch\"><span class=\"dot\" style=\"background:{}\"></span><span>{}</span><input type=\"checkbox\" data-color=\"{i}\" aria-label=\"Use {}\"{checked}></label>", tile.hex(), escape(tile.name), escape(tile.name));
    }
    element("exclusions")?.set_inner_html(&html);
    Ok(())
}

fn update_labels() -> Result<(), JsValue> {
    update_labels_with(false)
}

fn update_labels_with(state_busy: bool) -> Result<(), JsValue> {
    text("pad-v", &format!("{:.0}", number("pad")?))?;
    text(
        "separator_guard-v",
        &format!("{:.0}%", number("separator_guard")? * 100.0),
    )?;
    for id in ["contrast", "saturation", "gamma", "crop_zoom"] {
        text(&format!("{id}-v"), &format!("{:.2}×", number(id)?))?;
    }
    for id in [
        "crop_x",
        "crop_y",
        "hue_strength",
        "outline_strength",
        "neutral_cleanup",
        "dither_strength",
    ] {
        text(&format!("{id}-v"), &format!("{:.0}%", number(id)? * 100.0))?;
    }
    for id in ["hue_threshold", "secondary_threshold", "secondary_strength"] {
        text(&format!("{id}-v"), &format!("{:.1}%", number(id)? * 100.0))?;
    }
    for id in [
        "source_hue",
        "hue_tolerance",
        "secondary_hue",
        "secondary_tolerance",
    ] {
        text(&format!("{id}-v"), &format!("{:.0}°", number(id)?))?;
    }
    text("brightness-v", &format!("{:+.0}", number("brightness")?))?;
    // Do not reject partially typed dimensions until Build is submitted.
    if let (Ok(w), Ok(h)) = (number("width"), number("height")) {
        text(
            "size-note",
            &format!(
                "{w:.0} × {h:.0} = {:.0} pieces · about {:.1} × {:.1} cm",
                w * h,
                w * 0.8,
                h * 0.8
            ),
        )?;
    }
    let cropping = select("fit")?.value() == "crop";
    for id in ["crop_x", "crop_y", "crop_zoom"] {
        input(id)?.set_disabled(!cropping);
    }
    let hue_mode = select("hue_mode")?.value();
    hidden("remap-controls", hue_mode == "preserve")?;
    input("target_color")?.set_disabled(hue_mode != "target");
    input("dither_strength")?.set_disabled(!input("dither")?.checked());
    input("despeckle")?.set_disabled(input("dither")?.checked());
    input("secondary_target")?.set_disabled(select("secondary_mode")?.value() != "target");
    // Fitting lives in the preview footer next to the crop frame.
    select("fit")?.set_disabled(state_busy);
    Ok(())
}

fn apply_preset(state: &Shared, preset: &str, reset: bool) -> Result<(), JsValue> {
    let mut options = Options::default();
    let (palette_id, note) = match preset {
        "photo" => {
            options.order = Order::Smooth;
            options.outline_strength = 0.0;
            options.dither = true;
            options.white_background = false;
            options.neutral_cleanup = 0.0;
            options.despeckle = false;
            options.hue_remap.mode = HueMode::Preserve;
            options.secondary_remap.mode = HueMode::Preserve;
            (
                "mosaic-maker",
                "Smooth tonal matching and gentle dithering for photographs.",
            )
        }
        "grayscale" => {
            options.order = Order::Smooth;
            options.outline_strength = 0.0;
            options.saturation = 0.0;
            options.hue_remap.mode = HueMode::Preserve;
            options.secondary_remap.mode = HueMode::Preserve;
            (
                "monochrome",
                "A neutral portrait using only the six-color grayscale palette.",
            )
        }
        "yellow-accent" => {
            options.hue_remap.mode = HueMode::Target;
            (
                "mosaic-maker",
                "Turn pink/red accents into yellow while protecting paper and dark outlines.",
            )
        }
        _ => (
            "mosaic-maker",
            "Crisp shapes, protected tiny features, and the whole illustration in frame.",
        ),
    };
    if reset {
        state.borrow_mut().exclusions.clear();
        input("width")?.set_value(&options.width.to_string());
        input("height")?.set_value(&options.height.to_string());
    }
    select("preset")?.set_value(preset);
    select("palette")?.set_value(palette_id);
    select("order")?.set_value(if options.order == Order::Smooth {
        "smooth"
    } else {
        "sharp"
    });
    select("fit")?.set_value("contain");
    input("pad")?.set_value("255");
    select("hue_mode")?.set_value(match options.hue_remap.mode {
        HueMode::Auto => "auto",
        HueMode::Preserve => "preserve",
        HueMode::Grayscale => "grayscale",
        HueMode::Target => "target",
    });
    for (id, v) in [
        ("brightness", f64::from(options.brightness)),
        ("contrast", options.contrast),
        ("saturation", options.saturation),
        ("gamma", options.gamma),
        ("outline_strength", options.outline_strength),
        ("separator_guard", options.separator_guard),
        ("crop_x", options.crop_x),
        ("crop_y", options.crop_y),
        ("crop_zoom", options.zoom),
        ("source_hue", options.hue_remap.source_hue),
        ("hue_threshold", options.hue_remap.threshold),
        ("secondary_hue", options.secondary_remap.source_hue),
        ("secondary_tolerance", options.secondary_remap.tolerance),
        ("secondary_threshold", options.secondary_remap.threshold),
        ("secondary_strength", options.secondary_remap.strength),
        ("hue_tolerance", options.hue_remap.tolerance),
        ("hue_strength", options.hue_remap.strength),
        ("neutral_cleanup", options.neutral_cleanup),
        ("dither_strength", options.dither_strength),
    ] {
        input(id)?.set_value(&v.to_string());
    }
    input("target_color")?.set_value(&format!(
        "#{:02x}{:02x}{:02x}",
        options.hue_remap.target[0], options.hue_remap.target[1], options.hue_remap.target[2]
    ));
    for (id, checked) in [
        ("white_background", options.white_background),
        ("despeckle", options.despeckle),
        ("dither", options.dither),
        ("stage_previews", options.stage_previews),
    ] {
        input(id)?.set_checked(checked);
    }
    select("secondary_mode")?.set_value(if options.secondary_remap.mode == HueMode::Preserve {
        "preserve"
    } else {
        "auto"
    });
    input("secondary_target")?.set_value("#a0a5a9");
    text("preset-note", note)?;
    swatches(state)?;
    update_labels()?;
    mark_pending(state)?;
    refresh_crop_overlay(state)
}

fn read_options(state: &Shared) -> Result<(String, Options), JsValue> {
    let width = number("width")?;
    let height = number("height")?;
    if !(8.0..=128.0).contains(&width)
        || !(8.0..=128.0).contains(&height)
        || width.fract() != 0.0
        || height.fract() != 0.0
    {
        return Err(JsValue::from_str(
            "Choose whole-number dimensions from 8 to 128 studs.",
        ));
    }
    let palette_id = select("palette")?.value();
    let mut options = Options {
        width: width as usize,
        height: height as usize,
        brightness: number("brightness")? as i32,
        contrast: number("contrast")?,
        saturation: number("saturation")?,
        gamma: number("gamma")?,
        outline_strength: number("outline_strength")?,
        crop_x: number("crop_x")?,
        crop_y: number("crop_y")?,
        zoom: number("crop_zoom")?,
        neutral_cleanup: number("neutral_cleanup")?,
        dither_strength: number("dither_strength")?,
        white_background: input("white_background")?.checked(),
        despeckle: input("despeckle")?.checked(),
        dither: input("dither")?.checked(),
        order: if select("order")?.value() == "smooth" {
            Order::Smooth
        } else {
            Order::Sharp
        },
        pad: number("pad")?.clamp(0.0, 255.0) as u8,
        color_limit: number("color_limit")?.clamp(0.0, 16_384.0) as usize,
        separator_guard: number("separator_guard")?,
        fit: match select("fit")?.value().as_str() {
            "crop" => FitMode::Crop,
            "stretch" => FitMode::Stretch,
            _ => FitMode::Contain,
        },
        excluded: state
            .borrow()
            .exclusions
            .get(&palette_id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default(),
        stage_previews: input("stage_previews")?.checked(),
        ..Options::default()
    };
    options.hue_remap.mode = match select("hue_mode")?.value().as_str() {
        "preserve" => HueMode::Preserve,
        "grayscale" => HueMode::Grayscale,
        "target" => HueMode::Target,
        _ => HueMode::Auto,
    };
    options.hue_remap.source_hue = number("source_hue")?;
    options.hue_remap.tolerance = number("hue_tolerance")?;
    options.hue_remap.strength = number("hue_strength")?;
    options.hue_remap.threshold = number("hue_threshold")?;
    options.secondary_remap.mode = match select("secondary_mode")?.value().as_str() {
        "preserve" => HueMode::Preserve,
        "grayscale" => HueMode::Grayscale,
        "target" => HueMode::Target,
        _ => HueMode::Auto,
    };
    options.secondary_remap.source_hue = number("secondary_hue")?;
    options.secondary_remap.tolerance = number("secondary_tolerance")?;
    options.secondary_remap.threshold = number("secondary_threshold")?;
    options.secondary_remap.strength = number("secondary_strength")?;
    let secondary = value("secondary_target")?;
    if secondary.len() == 7 && secondary.is_ascii() {
        for i in 0..3 {
            options.secondary_remap.target[i] =
                u8::from_str_radix(&secondary[1 + i * 2..3 + i * 2], 16)
                    .map_err(|_| JsValue::from_str("Invalid second target color"))?;
        }
    }
    let hex = value("target_color")?;
    if hex.len() == 7 && hex.is_ascii() {
        for i in 0..3 {
            options.hue_remap.target[i] = u8::from_str_radix(&hex[1 + i * 2..3 + i * 2], 16)
                .map_err(|_| JsValue::from_str("Invalid accent color"))?;
        }
    }
    Ok((palette_id, options))
}

async fn yield_to_browser() -> Result<(), JsValue> {
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        if let Some(window) = web_sys::window() {
            if let Err(error) =
                window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 40)
            {
                let _ = reject.call1(&JsValue::NULL, &error);
            }
        } else {
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("Window unavailable"));
        }
    });
    JsFuture::from(promise).await?;
    Ok(())
}

async fn decode(file: &File) -> Result<(Raster, String), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("Window unavailable"))?;
    let bitmap: ImageBitmap = JsFuture::from(window.create_image_bitmap_with_blob(file)?)
        .await?
        .dyn_into()?;
    let (width, height) = (bitmap.width(), bitmap.height());
    if width == 0 || height == 0 {
        bitmap.close();
        return Err(JsValue::from_str("This image has no pixels."));
    }
    if u64::from(width) * u64::from(height) > 32_000_000 {
        bitmap.close();
        return Err(JsValue::from_str(
            "Please resize this image below 32 megapixels before opening it.",
        ));
    }
    // Bound the retained RGBA image to 16 MiB. Large source files remain local;
    // canvas downscales once, not on every subsequent build.
    let scale = (2048.0 / f64::from(width.max(height))).min(1.0);
    let w = (f64::from(width) * scale).round().max(1.0) as u32;
    let h = (f64::from(height) * scale).round().max(1.0) as u32;
    let canvas: HtmlCanvasElement = document()?.create_element("canvas")?.dyn_into()?;
    canvas.set_width(w);
    canvas.set_height(h);
    let result = (|| {
        let context: CanvasRenderingContext2d = canvas
            .get_context("2d")?
            .ok_or_else(|| JsValue::from_str("Canvas is unavailable"))?
            .dyn_into()?;
        context.draw_image_with_image_bitmap_and_dw_and_dh(
            &bitmap,
            0.0,
            0.0,
            f64::from(w),
            f64::from(h),
        )?;
        let pixels = context
            .get_image_data(0.0, 0.0, f64::from(w), f64::from(h))?
            .data()
            .0;
        // Use the bounded canvas for the original preview too; do not decode
        // the full source a second time in an <img> (especially large photos).
        let preview = canvas.to_data_url_with_type("image/png")?;
        Ok((
            Raster {
                width: w as usize,
                height: h as usize,
                pixels,
            },
            preview,
        ))
    })();
    bitmap.close();
    result
}

fn choose_file(state: &Shared, file: File) -> Result<(), JsValue> {
    if state.borrow().busy {
        return Ok(());
    }
    if !file.type_().is_empty() && !file.type_().starts_with("image/") {
        return Err(JsValue::from_str(
            "Please choose an image file (PNG, JPEG, WebP or another browser-supported format).",
        ));
    }
    if file.size() > 40.0 * 1024.0 * 1024.0 {
        return Err(JsValue::from_str("Please use an image smaller than 40 MB."));
    }
    hidden("error", true)?;
    set_busy(state, true, "Decoding image locally…")?;
    let state = Rc::clone(state);
    spawn_local(async move {
        let result = async {
            yield_to_browser().await?;
            let (raster, url) = decode(&file).await?;
            element("original")?
                .dyn_into::<HtmlImageElement>()?
                .set_src(&url);
            hidden("original", false)?;
            hidden("original-empty", true)?;
            {
                let app = Rc::clone(&state);
                let once = Closure::<dyn FnMut()>::new(move || {
                    let _ = refresh_crop_overlay(&app);
                });
                let image = element("original")?.dyn_into::<HtmlImageElement>()?;
                image.set_onload(Some(once.as_ref().unchecked_ref()));
                once.forget();
            }
            text(
                "filename",
                &format!(
                    "{} · {:.0} KB · local only",
                    file.name(),
                    file.size() / 1024.0
                ),
            )?;
            text(
                "source-size",
                &format!("{} × {} working pixels", raster.width, raster.height),
            )?;
            {
                let mut app = state.borrow_mut();
                if let Some(old) = app.source_url.replace(url) {
                    let _ = Url::revoke_object_url(&old);
                }
                if let Some(old) = app.mosaic_url.take() {
                    let _ = Url::revoke_object_url(&old);
                }
                app.file = Some(file);
                app.raster = Some(raster);
                app.mosaic = None;
                app.svg.clear();
                app.guide.clear();
                app.csv.clear();
            }
            hidden("outputs", true)?;
            hidden("output-empty", false)?;
            hidden("mosaic-image", true)?;
            hidden("mosaic-empty", false)?;
            input("zoom")?.set_disabled(true);
            text("result-label", "Ready to build")?;
            text("gridnote", "New image · ready to build")?;
            Ok::<_, JsValue>(())
        }
        .await;
        let _ = set_busy(
            &state,
            false,
            if result.is_ok() {
                "Image ready. Adjust your settings, then Build mosaic."
            } else {
                "Could not decode that image. Your previous work is unchanged."
            },
        );
        let _ = update_labels();
        if let Err(error) = result {
            show_error(error);
        }
    });
    Ok(())
}

fn blob_url(content: &str, mime: &str) -> Result<String, JsValue> {
    let parts = js_sys::Array::new();
    parts.push(&JsValue::from_str(content));
    let properties = BlobPropertyBag::new();
    properties.set_type(mime);
    let blob = Blob::new_with_str_sequence_and_options(&parts, &properties)?;
    Url::create_object_url_with_blob(&blob)
}

/// Render optional worker stage rasters into the collapsible stage section.
fn show_stages(stages: &StagePayload) -> Result<(), JsValue> {
    let payloads = [&stages.0, &stages.1, &stages.2, &stages.3];
    let names = ["fitted", "adjusted", "recolored", "tiles"];
    for (name, payload) in names.iter().zip(payloads) {
        let element = element(&format!("stage-{name}"))?;
        let raster = match payload {
            StageImage::Some {
                width,
                height,
                pixels,
            } => {
                text(
                    &format!("stage-{name}-size"),
                    &format!("· {width} × {height} px"),
                )?;
                Some((*width, *height, pixels))
            }
            StageImage::None => None,
        };
        match raster_data_url(raster.as_ref())? {
            Some(url) => {
                element.dyn_into::<HtmlImageElement>()?.set_src(&url);
                hidden(&format!("stage-{name}"), false)?;
            }
            None => {
                hidden(&format!("stage-{name}"), true)?;
            }
        }
    }
    if payloads
        .iter()
        .any(|p| matches!(p, StageImage::Some { .. }))
    {
        hidden("stages-card", false)?;
    } else {
        hidden("stages-card", true)?;
    }
    Ok(())
}

/// Encode a stage raster as a PNG data URL through a canvas (no uploads).
fn raster_data_url(payload: Option<&(usize, usize, &Vec<u8>)>) -> Result<Option<String>, JsValue> {
    let Some((width, height, pixels)) = payload else {
        return Ok(None);
    };
    if pixels.len() != width * height * 4 {
        return Ok(None);
    }
    let canvas: HtmlCanvasElement = document()?.create_element("canvas")?.dyn_into()?;
    canvas.set_width(*width as u32);
    canvas.set_height(*height as u32);
    let context: CanvasRenderingContext2d = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("Canvas is unavailable"))?
        .dyn_into()?;
    let data = ImageData::new_with_u8_clamped_array_and_sh(
        wasm_bindgen::Clamped(pixels.as_slice()),
        *width as u32,
        *height as u32,
    )?;
    context.put_image_data(&data, 0.0, 0.0)?;
    Ok(Some(canvas.to_data_url_with_type("image/png")?))
}

fn present(state: &Shared, mosaic: Mosaic, elapsed: f64) -> Result<(), JsValue> {
    let style = select("preview_style")?.value();
    let svg = if style == "flat" {
        render::flat_svg(&mosaic, 20)
    } else {
        render::stud_svg(&mosaic, 20)
    };
    let bottom_up = select("row_order")?.value() == "bottom";
    text(
        "row-note",
        if bottom_up {
            "Row 1 is the bottom. Build left to right, then upward."
        } else {
            "Row 1 is the top. Build left to right, then downward."
        },
    )?;
    let guide = render::guide_svg_order(&mosaic, 24, bottom_up);
    let url = blob_url(&svg, "image/svg+xml")?;
    element("mosaic-image")?
        .dyn_into::<HtmlImageElement>()?
        .set_src(&url);
    hidden("mosaic-image", false)?;
    hidden("mosaic-empty", true)?;
    input("zoom")?.set_disabled(false);
    input("zoom")?.set_value("100");
    apply_zoom()?;
    let csv = render::parts_csv(&mosaic);
    let parts = render::parts_list(&mosaic);
    let mut html = String::new();
    for part in &parts {
        let tile = mosaic.palette.iter().find(|tile| tile.name == part.name);
        let symbol = tile.map_or_else(|| "?".to_string(), crate::palette::TileColor::symbol);
        let _ = write!(html, "<div class=\"part\"><span class=\"color-number\">{symbol}</span><span class=\"dot\" style=\"background:{}\"></span><span>{}</span><strong>× {}</strong></div>", part.hex, escape(&part.name), part.count);
    }
    element("parts")?.set_inner_html(&html);
    text(
        "parts-total",
        &format!("· {} tiles / {} colors", mosaic.grid.len(), parts.len()),
    )?;
    let mut rows =
        String::from("<table class=\"build-table\"><thead><tr><th scope=\"col\">Row</th>");
    for x in 1..=mosaic.width {
        let _ = write!(rows, "<th scope=\"col\">{x}</th>");
    }
    rows.push_str("</tr></thead><tbody>");
    for (i, row) in render::build_rows_order(&mosaic, bottom_up)
        .iter()
        .enumerate()
    {
        let _ = write!(rows, "<tr><th scope=\"row\">{}</th>", i + 1);
        for (x, (name, hex)) in row.iter().enumerate() {
            let symbol = mosaic
                .palette
                .iter()
                .find(|tile| tile.name == name)
                .map_or_else(|| "?".to_string(), crate::palette::TileColor::symbol);
            let _ = write!(rows, "<td style=\"background:{hex}\" title=\"Row {}, column {}: {}\"><span>{symbol}</span></td>", i + 1, x + 1, escape(name));
        }
        rows.push_str("</tr>");
    }
    rows.push_str("</tbody></table>");
    element("rows")?.set_inner_html(&rows);
    text(
        "gridnote",
        &format!(
            "{} × {} · {} tiles",
            mosaic.width,
            mosaic.height,
            mosaic.grid.len()
        ),
    )?;
    text(
        "result-label",
        &format!("{} colors · {:.1}s", parts.len(), elapsed / 1000.0),
    )?;
    let mut metadata = format!(
        "{} × {} · {} colors · built in {:.1} s",
        mosaic.width,
        mosaic.height,
        parts.len(),
        elapsed / 1000.0
    );
    if !mosaic.overflow.is_empty() {
        metadata.push_str(&format!(
            " · ⚠ limit exceeded: {}",
            mosaic
                .overflow
                .iter()
                .map(|(index, excess)| format!("{} +{}", mosaic.palette[*index].name, excess))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    } else if input("color_limit").is_ok_and(|el| el.value() != "0") {
        metadata.push_str(" · color cap satisfied");
    }
    text("build-meta", &metadata)?;
    hidden("outputs", false)?;
    hidden("output-empty", true)?;
    let mut app = state.borrow_mut();
    if let Some(old) = app.mosaic_url.replace(url) {
        let _ = Url::revoke_object_url(&old);
    }
    app.elapsed = elapsed;
    app.mosaic = Some(mosaic);
    app.svg = svg;
    app.guide = guide;
    app.csv = csv;
    Ok(())
}

fn build(state: &Shared) -> Result<(), JsValue> {
    if state.borrow().busy {
        return Ok(());
    }
    hidden("error", true)?;
    if state.borrow().raster.is_none() {
        return Err(JsValue::from_str(
            "Choose an image before building your mosaic.",
        ));
    }
    let (palette_id, options) = read_options(state)?;
    let tiles = palette::all()
        .iter()
        .find(|p| p.0 == palette_id)
        .ok_or_else(|| JsValue::from_str("Choose a palette"))?
        .2;
    if options.excluded.len() >= tiles.len() {
        return Err(JsValue::from_str("Keep at least one color enabled."));
    }
    // Retain the source for future builds. Transfer a single copy to the worker.
    let request = {
        let app = state.borrow();
        let image = app
            .raster
            .as_ref()
            .ok_or_else(|| JsValue::from_str("Choose an image"))?;
        let pixels = js_sys::Uint8Array::from(image.pixels.as_slice());
        let array = js_sys::Array::new();
        array.push(
            &serde_json::to_string(&(image.width, image.height, &palette_id, &options))
                .map_err(|e| JsValue::from_str(&e.to_string()))?
                .into(),
        );
        array.push(&pixels.buffer());
        array
    };
    let config = web_sys::WorkerOptions::new();
    config.set_type(web_sys::WorkerType::Module);
    let worker = web_sys::Worker::new_with_options("./worker.js", &config)?;
    set_busy(
        state,
        true,
        "Building in a background worker… You can cancel.",
    )?;
    hidden("cancel", false)?;
    let job = {
        let mut app = state.borrow_mut();
        app.job += 1;
        app.job
    };
    let start = js_sys::Date::now();
    let weak = Rc::downgrade(state);
    let send = worker.clone();
    let handler =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            let Some(state) = weak.upgrade() else {
                return;
            };
            if state.borrow().job != job || state.borrow().worker.is_none() {
                return;
            }
            if event.data().as_string().as_deref() == Some("ready") {
                let transfer = js_sys::Array::new();
                transfer.push(&request.get(1));
                if let Err(error) = send.post_message_with_transfer(&request, &transfer) {
                    finish_worker(&state, Some(error));
                }
                return;
            }
            let result = (|| -> Result<(), JsValue> {
                let reply = js_sys::Array::from(&event.data());
                let body = reply
                    .get(1)
                    .as_string()
                    .ok_or_else(|| JsValue::from_str("Malformed worker response"))?;
                if reply.get(0).as_string().as_deref() != Some("result") {
                    return Err(body.into());
                }
                #[allow(clippy::type_complexity)] // worker wire format
                let (grid, width, height, colors, overflow, stage_payload): WorkerReply =
                    serde_json::from_str(&body).map_err(|e| JsValue::from_str(&e.to_string()))?;
                let palette = colors
                    .into_iter()
                    .map(|i| {
                        tiles
                            .get(i)
                            .copied()
                            .ok_or_else(|| JsValue::from_str("Invalid worker palette"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if grid.len() != width * height || grid.iter().any(|&i| i >= palette.len()) {
                    return Err("Invalid worker grid".into());
                }
                present(
                    &state,
                    Mosaic {
                        grid,
                        width,
                        height,
                        palette,
                        overflow,
                    },
                    js_sys::Date::now() - start,
                )?;
                show_stages(&stage_payload)
            })();
            finish_worker(&state, result.err());
        });
    let weak = Rc::downgrade(state);
    let error = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
        event.prevent_default();
        if let Some(state) = weak.upgrade() {
            if state.borrow().job == job {
                finish_worker(&state,Some("Background worker failed to load or run. Retry the build or reload the app.".into()));
            }
        }
    });
    let weak = Rc::downgrade(state);
    let timer = Closure::<dyn FnMut()>::new(move || {
        if let Some(state) = weak.upgrade() {
            if state.borrow().job == job {
                finish_worker(
                    &state,
                    Some("Build timed out. Try a smaller image or grid.".into()),
                );
            }
        }
    });
    worker.set_onmessage(Some(handler.as_ref().unchecked_ref()));
    worker.set_onerror(Some(error.as_ref().unchecked_ref()));
    worker.set_onmessageerror(Some(error.as_ref().unchecked_ref()));
    let timeout = web_sys::window()
        .ok_or_else(|| JsValue::from_str("Window unavailable"))?
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            timer.as_ref().unchecked_ref(),
            120_000,
        )?;
    let mut app = state.borrow_mut();
    app.worker = Some(worker);
    app.worker_message = Some(handler);
    app.worker_error = Some(error);
    app.timeout = Some(timeout);
    app.timer_callback = Some(timer);
    Ok(())
}

fn finish_worker(state: &Shared, error: Option<JsValue>) {
    {
        let mut app = state.borrow_mut();
        if let Some(worker) = app.worker.take() {
            worker.set_onmessage(None);
            worker.set_onerror(None);
            worker.set_onmessageerror(None);
            worker.terminate();
        }
        if let Some(id) = app.timeout.take() {
            if let Some(window) = web_sys::window() {
                window.clear_timeout_with_handle(id);
            }
        }
        // Keep closures until the next job: completion may be executing one.
    }
    let _ = hidden("cancel", true);
    let _ = set_busy(
        state,
        false,
        if error.is_some() {
            "Build failed. Try again."
        } else {
            "Mosaic ready. Save your mosaic, parts or guide."
        },
    );
    if let Some(error) = error {
        show_error(error);
    }
}

fn apply_zoom() -> Result<(), JsValue> {
    let zoom = number("zoom")?;
    element("mosaic-image")?
        .dyn_into::<HtmlElement>()?
        .style()
        .set_property("width", &format!("{zoom}%"))?;
    text("zoom-v", &format!("{zoom:.0}%"))
}

/// Frame rect of the sliders' current options, for the given raster.
/// Errors (e.g. a half-typed width field) leave the drawn frame untouched —
/// typing must never pop error boxes.
fn options_frame(state: &Shared, raster: &Raster) -> Result<Option<CropFrame>, JsValue> {
    match read_options(state) {
        Ok((_, options)) => {
            let (x, y, w, h) = crop_frame(raster, options.width, options.height, &options);
            Ok(Some(CropFrame { x, y, w, h }))
        }
        Err(_) => Ok(None),
    }
}

/// Redraw the overlay from the current state. Called after decode, preset
/// changes, slider input, builds and layout-affecting resizes.
fn refresh_crop_overlay(state: &Shared) -> Result<(), JsValue> {
    let raster = state.borrow().raster.clone();
    let Some(raster) = raster else {
        state.borrow_mut().crop.drawn = None;
        hidden("crop-note", true)?;
        hidden("crop-wrap", true)?;
        return Ok(());
    };
    // The wrap carries the picture itself: size it on every refresh,
    // whatever the fitting mode is, so the image always fits the well.
    crop_layout(&raster)?;
    hidden("crop-wrap", false)?;
    let cropping = select("fit")?.value() == "crop";
    hidden("crop-note", !cropping)?;
    hidden("crop-overlay", !cropping)?;
    // Recompute from the controls unless a pointer gesture owns the frame.
    if cropping && state.borrow().crop.drag.is_none() {
        if let Some(frame) = options_frame(state, &raster)? {
            draw_crop_frame(state, &raster, &frame)?;
        }
    }
    Ok(())
}

/// Paint one frame: overlay box, dimmed outside, corner handles. All
/// coordinates are wrap-relative; the wrap tracks the picture exactly.
fn draw_crop_frame(state: &Shared, raster: &Raster, frame: &CropFrame) -> Result<(), JsValue> {
    let layout = crop_layout(raster)?;
    let css = layout.to_css(frame);
    hidden("crop-overlay", false)?;
    let frame_el: HtmlElement = element("crop-frame")?.dyn_into()?;
    let style = frame_el.style();
    style.set_property("left", &format!("{:.2}px", css.x))?;
    style.set_property("top", &format!("{:.2}px", css.y))?;
    style.set_property("width", &format!("{:.2}px", css.w))?;
    style.set_property("height", &format!("{:.2}px", css.h))?;
    for (name, hx, hy) in [
        ("nw", css.x, css.y),
        ("ne", css.x + css.w, css.y),
        ("sw", css.x, css.y + css.h),
        ("se", css.x + css.w, css.y + css.h),
    ] {
        let handle: HtmlElement = element(&format!("crop-handle-{name}"))?.dyn_into()?;
        let style = handle.style();
        style.set_property("left", &format!("{:.2}px", hx))?;
        style.set_property("top", &format!("{:.2}px", hy))?;
    }
    state.borrow_mut().crop.drawn = Some(*frame);
    Ok(())
}

/// Commit a frame: write sliders, keep the picture untouched, mark outputs
/// stale. The frame is already grid-aspect in source pixels. Slider reads
/// can fail on a half-typed number field; gestures then do nothing rather
/// than spamming errors.
fn apply_crop_frame(state: &Shared, raster: &Raster, frame: &CropFrame) -> Result<(), JsValue> {
    let Ok((_, options)) = read_options(state) else {
        return Ok(());
    };
    let next = crop_frame_options(
        raster,
        options.width,
        options.height,
        &options,
        (frame.x, frame.y, frame.w, frame.h),
    );
    input("crop_x")?.set_value(&next.crop_x.to_string());
    input("crop_y")?.set_value(&next.crop_y.to_string());
    input("crop_zoom")?.set_value(&next.zoom.to_string());
    update_labels()?;
    mark_pending(state)
}

/// Start a gesture on the overlay or a corner handle: remember what was
/// grabbed (corner resize, or whole-frame pan) and where the pointer began.
fn crop_gesture_start(state: &Shared, event: &Event) -> Result<(), JsValue> {
    if state.borrow().busy {
        return Ok(());
    }
    let raster = state
        .borrow()
        .raster
        .clone()
        .ok_or_else(|| JsValue::from_str("Choose an image first"))?;
    // Handles sit inside the overlay, so gestures bubble up to it: without
    // this gate one press would start two gestures.
    if state.borrow().crop.drag.is_some() {
        return Ok(());
    }
    let target: HtmlElement = match event.target() {
        Some(t) => t.dyn_into()?,
        None => return Ok(()),
    };
    let corner = match target.get_attribute("data-handle") {
        Some(name) => match CropHandle::of(&name) {
            Some(c) => Some(c),
            None => return Ok(()),
        },
        // Pan only when the overlay itself was grabbed, not its handles.
        None if target.get_attribute("id").as_deref() == Some("crop-overlay") => None,
        _ => return Ok(()),
    };
    event.prevent_default();
    let pointer = event
        .dyn_ref::<PointerEvent>()
        .ok_or_else(|| JsValue::from_str("Pointer event expected"))?;
    let layout = crop_layout(&raster)?;
    let rect = wrap_rect()?;
    let frame = state
        .borrow()
        .crop
        .drawn
        .ok_or_else(|| JsValue::from_str("Crop frame is not ready"))?;
    let (px, py) = layout.clamped_source(&rect, pointer.client_x().into(), pointer.client_y().into());
    // Corners: store the offset from the grabbed corner. Pans: the grab
    // point inside the frame; movement shifts the frame by the same delta.
    let (grab_x, grab_y) = match corner {
        Some(c) => {
            let (cx, cy) = corner_point(&frame, c);
            (px - cx, py - cy)
        }
        None => (px - frame.x, py - frame.y),
    };
    state.borrow_mut().crop.drag = Some(CropDrag {
        corner,
        grab_x,
        grab_y,
        origin: frame,
    });
    let _ = target.set_pointer_capture(pointer.pointer_id());
    Ok(())
}

/// Pointer move with an active gesture: resize from the corner or pan the
/// whole frame, then commit and repaint.
fn crop_gesture_move(state: &Shared, event: &Event) -> Result<(), JsValue> {
    let Some(drag) = state.borrow().crop.drag else {
        return Ok(());
    };
    if state.borrow().busy {
        state.borrow_mut().crop.drag = None;
        return Ok(());
    }
    let target: HtmlElement = match event.target() {
        Some(t) => t.dyn_into()?,
        None => return Ok(()),
    };
    // Only the captured element drives its gesture (handles resize, the
    // overlay pans); ignore any other target that happens to bubble.
    let wanted = match drag.corner {
        Some(_) => target.get_attribute("data-handle").is_some(),
        None => target.get_attribute("id").as_deref() == Some("crop-overlay"),
    };
    if !wanted {
        return Ok(());
    }
    event.prevent_default();
    let raster = state
        .borrow()
        .raster
        .clone()
        .ok_or_else(|| JsValue::from_str("Choose an image first"))?;
    let pointer = event
        .dyn_ref::<PointerEvent>()
        .ok_or_else(|| JsValue::from_str("Pointer event expected"))?;
    let layout = crop_layout(&raster)?;
    let rect = wrap_rect()?;
    let (px, py) = layout.clamped_source(&rect, pointer.client_x().into(), pointer.client_y().into());
    let frame = match drag.corner {
        // The grabbed corner follows the pointer minus the grab offset.
        Some(corner) => {
            anchored_frame(&drag.origin, corner, px - drag.grab_x, py - drag.grab_y, &raster)
        }
        None => {
            let start = drag.origin;
            let x = (px - drag.grab_x).clamp(0.0, (raster.width as f64 - start.w).max(0.0));
            let y = (py - drag.grab_y).clamp(0.0, (raster.height as f64 - start.h).max(0.0));
            CropFrame { x, y, ..start }
        }
    };
    apply_crop_frame(state, &raster, &frame)?;
    draw_crop_frame(state, &raster, &frame)
}

/// Gesture finished: drop the drag state and release pointer capture.
fn crop_gesture_end(state: &Shared, event: &Event) -> Result<(), JsValue> {
    if state.borrow().crop.drag.is_none() {
        return Ok(());
    }
    event.prevent_default();
    state.borrow_mut().crop.drag = None;
    if let Some(target) = event.target().and_then(|t| t.dyn_into::<HtmlElement>().ok()) {
        if let Some(pointer) = event.dyn_ref::<PointerEvent>() {
            let _ = target.release_pointer_capture(pointer.pointer_id());
        }
    }
    Ok(())
}

/// Zoom by wheel over the overlay: shrink/grow the frame around the pointer.
fn crop_wheel(state: &Shared, event: &Event) -> Result<(), JsValue> {
    let Some(target) = event.target() else {
        return Ok(());
    };
    let overlay: HtmlElement = target.dyn_into()?;
    if overlay.get_attribute("id").as_deref() != Some("crop-overlay") {
        return Ok(());
    }
    event.prevent_default();
    let raster = state
        .borrow()
        .raster
        .clone()
        .ok_or_else(|| JsValue::from_str("Choose an image first"))?;
    let wheel = event
        .dyn_ref::<WheelEvent>()
        .ok_or_else(|| JsValue::from_str("Wheel event expected"))?;
    let layout = crop_layout(&raster)?;
    let current = state
        .borrow()
        .crop
        .drawn
        .ok_or_else(|| JsValue::from_str("Crop frame is not ready"))?;
    let factor = if wheel.delta_y() > 0.0 { 1.1 } else { 1.0 / 1.1 };
    // Keep the point under the wheel visually fixed while resizing: with
    // f = (pointer - frame) clamped into the frame, new x = f - f·(w'/w).
    let rect = wrap_rect()?;
    let (px, py) = layout.to_source(&rect, wheel.client_x().into(), wheel.client_y().into());
    let focus_x = (px - current.x).clamp(0.0, current.w);
    let focus_y = (py - current.y).clamp(0.0, current.h);
    let aspect = current.w / current.h;
    // Grow/shrink on the dominant axis, then re-derive the pair for grid
    // aspect, clamped to the image bounds on both axes.
    let mut w = current.w * factor;
    let max_w = raster.width as f64;
    let max_h = raster.height as f64;
    let min_side = cell_size(&raster);
    w = w.clamp(min_side, max_w);
    let mut h = (w / aspect).clamp(min_side, max_h);
    if h * aspect > max_w {
        w = max_h * aspect;
        if w > max_w {
            w = max_w;
        }
        h = w / aspect;
    }
    w = (h * aspect).min(max_w);
    let x = (current.x + focus_x * (1.0 - w / current.w)).clamp(0.0, (max_w - w).max(0.0));
    let y = (current.y + focus_y * (1.0 - h / current.h)).clamp(0.0, (max_h - h).max(0.0));
    let frame = CropFrame { x, y, w, h };
    apply_crop_frame(state, &raster, &frame)?;
    draw_crop_frame(state, &raster, &frame)
}

/// Arrow-key nudge of a focused corner handle, one source pixel per press
/// (ten with Shift). Keeps grid aspect like dragging.
fn crop_handle_key(state: &Shared, handle_name: &str, event: &Event) -> Result<(), JsValue> {
    let Some(handle) = CropHandle::of(handle_name) else {
        return Ok(());
    };
    let key_event = event
        .dyn_ref::<KeyboardEvent>()
        .ok_or_else(|| JsValue::from_str("Keyboard event expected"))?;
    let Some((dx, dy)) = handle.moved(&key_event.key()) else {
        return Ok(());
    };
    event.prevent_default();
    let raster = state
        .borrow()
        .raster
        .clone()
        .ok_or_else(|| JsValue::from_str("Choose an image first"))?;
    let frame = state
        .borrow()
        .crop
        .drawn
        .ok_or_else(|| JsValue::from_str("Crop frame is not ready"))?;
    let step = if key_event.shift_key() { 10.0 } else { 1.0 };
    let (corner_x, corner_y) = match handle {
        CropHandle::Nw => (frame.x + dx * step, frame.y + dy * step),
        CropHandle::Ne => (frame.x + frame.w + dx * step, frame.y + dy * step),
        CropHandle::Sw => (frame.x + dx * step, frame.y + frame.h + dy * step),
        CropHandle::Se => (frame.x + frame.w + dx * step, frame.y + frame.h + dy * step),
    };
    let next = anchored_frame(&frame, handle, corner_x, corner_y, &raster);
    apply_crop_frame(state, &raster, &next)?;
    draw_crop_frame(state, &raster, &next)
}

/// Smallest sensible crop side: one grid cell in source pixels.
fn cell_size(raster: &Raster) -> f64 {
    let cols = raster.width as f64 / 16.0;
    let rows = raster.height as f64 / 16.0;
    let cells = cols.max(rows).ceil().max(1.0);
    (raster.width as f64 / cells).max(raster.height as f64 / cells).max(1.0)
}

/// Frame after moving `handle`'s corner to a point: opposite corner pinned,
/// grid aspect preserved, kept inside the image with a one-cell minimum.
fn anchored_frame(
    origin: &CropFrame,
    handle: CropHandle,
    corner_x: f64,
    corner_y: f64,
    raster: &Raster,
) -> CropFrame {
    let aspect = origin.w / origin.h;
    // Keep at least one grid cell: useful on small images, where a fixed
    // source-pixel floor could swallow the whole frame.
    let min_side = cell_size(raster);
    // Desired half-extents from the pinned corner.
    let (px, py) = match handle {
        CropHandle::Nw => (origin.x + origin.w, origin.y + origin.h),
        CropHandle::Ne => (origin.x, origin.y + origin.h),
        CropHandle::Sw => (origin.x + origin.w, origin.y),
        CropHandle::Se => (origin.x, origin.y),
    };
    let dx = (corner_x - px) * match handle {
        CropHandle::Nw | CropHandle::Sw => 1.0,
        _ => -1.0,
    };
    let dy = (corner_y - py) * match handle {
        CropHandle::Nw | CropHandle::Ne => 1.0,
        _ => -1.0,
    };
    // Choose the extent: the dominant axis keeps grid aspect.
    let mut w = dx.abs().max(dy.abs() * aspect);
    w = w.clamp(min_side, raster.width as f64);
    let mut h = w / aspect;
    if h > raster.height as f64 {
        h = raster.height as f64;
        w = (h * aspect).min(raster.width as f64);
    }
    // The opposite corner stays pinned: it is the fixed corner of the new
    // frame (Se pins under an Nw drag, Sw under Ne, etc.).
    let (x, y) = match handle {
        CropHandle::Nw => (px - w, py - h),
        CropHandle::Ne => (px, py - h),
        CropHandle::Sw => (px - w, py),
        CropHandle::Se => (px, py),
    };
    CropFrame {
        x: x.clamp(0.0, (raster.width as f64 - w).max(0.0)),
        y: y.clamp(0.0, (raster.height as f64 - h).max(0.0)),
        w,
        h,
    }
}

/// Wire all interactive-crop listeners. Fixed closures, like the rest of start().
fn listen_crop(state: &Shared) -> Result<(), JsValue> {
    for name in ["nw", "ne", "sw", "se"] {
        let app = Rc::clone(state);
        let handle = name.to_string();
        listen(&format!("crop-handle-{handle}"), "pointerdown", move |e| crop_gesture_start(&app, &e))?;
        let app = Rc::clone(state);
        let handle = name.to_string();
        listen(&format!("crop-handle-{handle}"), "pointermove", move |e| crop_gesture_move(&app, &e))?;
        let app = Rc::clone(state);
        let handle = name.to_string();
        listen(&format!("crop-handle-{handle}"), "pointerup", move |e| crop_gesture_end(&app, &e))?;
        let app = Rc::clone(state);
        let handle = name.to_string();
        listen(&format!("crop-handle-{handle}"), "keydown", move |e| crop_handle_key(&app, &handle, &e))?;
    }
    let app = Rc::clone(state);
    listen("crop-overlay", "pointerdown", move |e| crop_gesture_start(&app, &e))?;
    let app = Rc::clone(state);
    listen("crop-overlay", "pointermove", move |e| crop_gesture_move(&app, &e))?;
    let app = Rc::clone(state);
    listen("crop-overlay", "pointerup", move |e| crop_gesture_end(&app, &e))?;
    let app = Rc::clone(state);
    listen("crop-overlay", "wheel", move |e| crop_wheel(&app, &e))?;
    Ok(())
}

/// Corner of the crop frame being dragged or key-nudged.
#[derive(Clone, Copy, PartialEq)]
enum CropHandle {
    Nw,
    Ne,
    Sw,
    Se,
}

impl CropHandle {
    fn of(handle: &str) -> Option<CropHandle> {
        match handle {
            "nw" => Some(CropHandle::Nw),
            "ne" => Some(CropHandle::Ne),
            "sw" => Some(CropHandle::Sw),
            "se" => Some(CropHandle::Se),
            _ => None,
        }
    }
    /// Key nudge direction: arrows move the dragged corner.
    fn moved(self, key: &str) -> Option<(f64, f64)> {
        let (x, y) = match key {
            "ArrowLeft" => (-1.0, 0.0),
            "ArrowRight" => (1.0, 0.0),
            "ArrowUp" => (0.0, -1.0),
            "ArrowDown" => (0.0, 1.0),
            _ => return None,
        };
        Some((x, y))
    }
}

/// Cumulative layout for one overlay redraw. The wrap is sized in JS to the
/// letterboxed picture area (the object-fit box), so it always fits the well
/// and every overlay child hit-tests normally.
struct CropLayout {
    /// Source pixels per CSS pixel.
    scale: f64,
}

impl CropLayout {
    /// Where the current source frame sits, in wrap CSS pixels.
    fn to_css(&self, f: &CropFrame) -> CropFrame {
        CropFrame {
            x: f.x * self.scale,
            y: f.y * self.scale,
            w: f.w * self.scale,
            h: f.h * self.scale,
        }
    }
    /// Convert a pointer position (wrap-relative) into source pixels.
    fn to_source(&self, rect: &CropFrame, client_x: f64, client_y: f64) -> (f64, f64) {
        (
            (client_x - rect.x) / self.scale,
            (client_y - rect.y) / self.scale,
        )
    }
    /// Clamp a pointer position to the wrap, then map to source pixels.
    fn clamped_source(&self, rect: &CropFrame, client_x: f64, client_y: f64) -> (f64, f64) {
        let x = client_x.clamp(rect.x, rect.x + rect.w);
        let y = client_y.clamp(rect.y, rect.y + rect.h);
        self.to_source(rect, x, y)
    }
}

/// The three artifacts the studio can save, sharing one download path.
#[derive(Clone, Copy)]
enum Export {
    Mosaic,
    Guide,
    Parts,
}

fn element_rect(id: &str) -> Result<CropFrame, JsValue> {
    let rect = element(id)?.get_bounding_client_rect();
    Ok(CropFrame {
        x: rect.x(),
        y: rect.y(),
        w: rect.width(),
        h: rect.height(),
    })
}

/// A corner's position in source pixels.
fn corner_point(frame: &CropFrame, corner: CropHandle) -> (f64, f64) {
    match corner {
        CropHandle::Nw => (frame.x, frame.y),
        CropHandle::Ne => (frame.x + frame.w, frame.y),
        CropHandle::Sw => (frame.x, frame.y + frame.h),
        CropHandle::Se => (frame.x + frame.w, frame.y + frame.h),
    }
}

/// The wrap's live position, for pointer mapping.
fn wrap_rect() -> Result<CropFrame, JsValue> {
    element_rect("crop-wrap")
}

/// Size the wrap to the picture's object-fit area inside the well and return
/// the resulting layout. Called on every paint so window resizes and media
/// queries stay exact.
fn crop_layout(raster: &Raster) -> Result<CropLayout, JsValue> {
    let well: HtmlElement = element("original-well")?.dyn_into()?;
    let cw = f64::from(well.client_width());
    let ch = f64::from(well.client_height());
    if cw < 1.0 || ch < 1.0 || raster.width == 0 || raster.height == 0 {
        return Err(JsValue::from_str("Image layout is not ready yet"));
    }
    let scale = (cw / raster.width as f64).min(ch / raster.height as f64);
    let pw = raster.width as f64 * scale;
    let ph = raster.height as f64 * scale;
    let wrap: HtmlElement = element("crop-wrap")?.dyn_into()?;
    let style = wrap.style();
    style.set_property("width", &format!("{:.2}px", pw))?;
    style.set_property("height", &format!("{:.2}px", ph))?;
    style.set_property("left", &format!("{:.2}px", (cw - pw) * 0.5))?;
    style.set_property("top", &format!("{:.2}px", (ch - ph) * 0.5))?;
    Ok(CropLayout { scale })
}

fn download(state: &Shared, which: Export) -> Result<(), JsValue> {
    let app = state.borrow();
    if app.mosaic.is_none() {
        return Err(JsValue::from_str("Build a mosaic before downloading."));
    }
    let (content, mime, name) = match which {
        Export::Mosaic => (&app.svg, "image/svg+xml", "mosaic.svg"),
        Export::Guide => (&app.guide, "image/svg+xml", "mosaic-guide.svg"),
        Export::Parts => (&app.csv, "text/csv;charset=utf-8", "mosaic-parts.csv"),
    };
    let url = blob_url(content, mime)?;
    let link: HtmlAnchorElement = document()?.create_element("a")?.dyn_into()?;
    link.set_href(&url);
    link.set_download(name);
    link.style().set_property("display", "none")?;
    document()?
        .body()
        .ok_or_else(|| JsValue::from_str("Page unavailable"))?
        .append_child(&link)?;
    link.click();
    link.remove();
    // Allow the browser to consume the Blob before releasing the temporary URL.
    drop(app);
    spawn_local(async move {
        let _ = yield_to_browser().await;
        let _ = Url::revoke_object_url(&url);
    });
    Ok(())
}

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    if web_sys::window().is_none() {
        return crate::worker::start();
    }
    spawn_local(async {
        let result = async {
            let window = web_sys::window().ok_or_else(||JsValue::from_str("Window unavailable"))?;
            let container = window.navigator().service_worker();
            JsFuture::from(container.register("./service-worker.js")).await?;
            JsFuture::from(container.ready()?).await?;
            text("offline-status", "Ready for offline use. Install Mosaic Studio from your browser menu. Only app assets are cached; images stay in memory.")
        }.await;
        if result.is_err() {
            let _=text("offline-status","Offline setup unavailable. Online conversion still works locally; offline installation needs HTTPS or localhost.");
        }
    });
    let state = Rc::new(RefCell::new(App::default()));
    populate_palettes()?;
    apply_preset(&state, "artwork", true)?;
    let app = Rc::clone(&state);
    listen("match-aspect", "click", move |_| {
        let height = {
            let state = app.borrow();
            let image = state
                .raster
                .as_ref()
                .ok_or_else(|| JsValue::from_str("Choose an image first"))?;
            (number("width")? * image.height as f64 / image.width as f64)
                .round()
                .clamp(8.0, 128.0)
        };
        input("height")?.set_value(&height.to_string());
        update_labels()?;
        mark_pending(&app)
    })?;
    let app = Rc::clone(&state);
    listen("cancel", "click", move |_| {
        app.borrow_mut().job += 1;
        finish_worker(&app, None);
        text(
            "status",
            "Build cancelled. Previous result retained; change settings and rebuild.",
        )
    })?;
    let app = Rc::clone(&state);
    listen("row_order", "change", move |_| {
        let (mosaic, elapsed) = {
            let mut state = app.borrow_mut();
            (state.mosaic.take(), state.elapsed)
        };
        if let Some(mosaic) = mosaic {
            present(&app, mosaic, elapsed)?;
        }
        Ok(())
    })?;
    let app = Rc::clone(&state);
    listen("image", "change", move |_| {
        if let Some(file) = input("image")?.files().and_then(|f| f.get(0)) {
            choose_file(&app, file)?;
        }
        Ok(())
    })?;
    listen("drop", "dragover", |event| {
        event.prevent_default();
        element("drop")?.class_list().add_1("dragging")?;
        Ok(())
    })?;
    listen("drop", "dragleave", |_| {
        element("drop")?.class_list().remove_1("dragging")?;
        Ok(())
    })?;
    let app = Rc::clone(&state);
    listen("drop", "drop", move |event| {
        event.prevent_default();
        element("drop")?.class_list().remove_1("dragging")?;
        let event: web_sys::DragEvent = event.dyn_into()?;
        if let Some(file) = event
            .data_transfer()
            .and_then(|d| d.files())
            .and_then(|f| f.get(0))
        {
            choose_file(&app, file)?;
        }
        Ok(())
    })?;
    let app = Rc::clone(&state);
    listen("form", "submit", move |event| {
        event.prevent_default();
        build(&app)
    })?;
    let app = Rc::clone(&state);
    listen("preset", "change", move |_| {
        apply_preset(&app, &select("preset")?.value(), false)
    })?;
    let app = Rc::clone(&state);
    listen("reset", "click", move |_| {
        hidden("error", true)?;
        apply_preset(&app, "artwork", true)
    })?;
    let app = Rc::clone(&state);
    listen("palette", "change", move |_| {
        swatches(&app)?;
        mark_pending(&app)
    })?;
    let app = Rc::clone(&state);
    listen("exclusions", "change", move |event| {
        let Some(target) = event
            .target()
            .and_then(|t| t.dyn_into::<HtmlInputElement>().ok())
        else {
            return Ok(());
        };
        let Some(index) = target
            .get_attribute("data-color")
            .and_then(|s| s.parse::<usize>().ok())
        else {
            return Ok(());
        };
        let id = select("palette")?.value();
        let count = palette::all()
            .iter()
            .find(|p| p.0 == id)
            .map_or(0, |p| p.2.len());
        {
            let mut app = app.borrow_mut();
            let exclusions = app.exclusions.entry(id).or_default();
            if target.checked() {
                exclusions.remove(&index);
            } else if exclusions.len() + 1 >= count {
                target.set_checked(true);
                return Err(JsValue::from_str("Keep at least one color enabled."));
            } else {
                exclusions.insert(index);
            }
        }
        hidden("error", true)?;
        mark_pending(&app)
    })?;
    // Select controls fire `change` consistently across browser engines.
    let app = Rc::clone(&state);
    listen("settings", "change", move |_| {
        update_labels()?;
        mark_pending(&app)?;
        refresh_crop_overlay(&app)
    })?;
    let app = Rc::clone(&state);
    listen("settings", "input", move |event| {
        if event
            .target()
            .and_then(|t| t.dyn_into::<Element>().ok())
            .is_some_and(|el| el.id() == "image")
        {
            return Ok(());
        }
        update_labels()?;
        mark_pending(&app)?;
        refresh_crop_overlay(&app)
    })?;
    listen_crop(&state)?;
    // The frame must track the picture when the window or preview resizes.
    {
        let app = Rc::clone(&state);
        let callback = Closure::<dyn FnMut()>::new(move || {
            if let Err(error) = refresh_crop_overlay(&app) {
                web_sys::console::error_1(&error);
            }
        });
        web_sys::window()
            .ok_or_else(|| JsValue::from_str("Window unavailable"))?
            .add_event_listener_with_callback("resize", callback.as_ref().unchecked_ref())?;
        callback.forget();
    }
    listen("zoom", "input", |_| apply_zoom())?;
    let app = Rc::clone(&state);
    listen("fit", "change", move |_| {
        update_labels()?;
        mark_pending(&app)?;
        refresh_crop_overlay(&app)
    })?;
    let app = Rc::clone(&state);
    listen("export-svg", "click", move |_| {
        download(&app, Export::Mosaic)
    })?;
    let app = Rc::clone(&state);
    listen("export-guide", "click", move |_| {
        download(&app, Export::Guide)
    })?;
    let app = Rc::clone(&state);
    listen("export-csv", "click", move |_| {
        download(&app, Export::Parts)
    })?;
    listen("print", "click", |_| {
        web_sys::window()
            .ok_or_else(|| JsValue::from_str("Window unavailable"))?
            .print()
    })?;
    set_busy(
        &state,
        false,
        "Ready. Choose an image to begin — everything stays on your device.",
    )?;
    Ok(())
}
