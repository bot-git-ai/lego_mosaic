// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License
//! Dedicated-worker protocol. No pixel conversion runs on the window thread.
use crate::{
    mosaic::{self, Options, Raster},
    palette,
};
use wasm_bindgen::{prelude::*, JsCast};
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

pub fn start() -> Result<(), JsValue> {
    let scope: DedicatedWorkerGlobalScope = js_sys::global().dyn_into()?;
    let target = scope.clone();
    let handler = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let run = || -> Result<String, String> {
            let array = js_sys::Array::from(&event.data());
            let (width, height, id, options): (usize, usize, String, Options) =
                serde_json::from_str(&array.get(0).as_string().ok_or("Missing request")?)
                    .map_err(|e| e.to_string())?;
            let pixels = js_sys::Uint8Array::new(&array.get(1)).to_vec();
            if width == 0
                || height == 0
                || width
                    .checked_mul(height)
                    .is_none_or(|n| n > 32_000_000 || n * 4 != pixels.len())
            {
                return Err("Invalid worker image dimensions".into());
            }
            let tiles = palette::all()
                .iter()
                .find(|p| p.0 == id)
                .ok_or("Unknown palette")?
                .2;
            let result = mosaic::convert(
                &Raster {
                    width,
                    height,
                    pixels,
                },
                tiles,
                &options,
            );
            let colors: Vec<usize> = result
                .palette
                .iter()
                .map(|tile| {
                    tiles
                        .iter()
                        .position(|t| t.id() == tile.id())
                        .expect("subset palette")
                })
                .collect();
            serde_json::to_string(&(
                result.grid,
                result.width,
                result.height,
                colors,
                result.overflow,
            ))
            .map_err(|e| e.to_string())
        };
        let message = js_sys::Array::new();
        match run() {
            Ok(result) => {
                message.push(&"result".into());
                message.push(&result.into());
            }
            Err(error) => {
                message.push(&"error".into());
                message.push(&error.into());
            }
        }
        let _ = target.post_message(&message);
    });
    scope.set_onmessage(Some(handler.as_ref().unchecked_ref()));
    handler.forget(); // Lives exactly as long as this worker, terminated after each job.
    scope.post_message(&JsValue::from_str("ready"))
}
