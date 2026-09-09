//! <Heatmap> — global nested squarified treemap (Finviz-style), sectors 2D by
//! sqrt(total cap), stocks 2D by cap. Wrapped in a zoomable viewport: a zoom
//! slider scales the whole board (CSS transform) and the viewport scrolls, so
//! you can zoom into small sectors to read them. Colors react to the timeframe;
//! hover = brighten + pop; tooltip = symbol · cap · %change.

use alpaca_core::data::{color, mock_sectors, pct_change, Timeframe};
use alpaca_core::layout::{layout, StockTile};
use leptos::prelude::*;

const W: f64 = 1000.0; // canvas width (px)
const H: f64 = 680.0; // canvas height (px)
const GAP: f64 = 4.0; // inset inside each sector box
const HEADER: f64 = 18.0; // sector label strip height
const TILE_GAP: f64 = 2.0; // visual gap between tiles

#[component]
pub fn Heatmap(tf_idx: ReadSignal<usize>) -> impl IntoView {
    let boxes = layout(mock_sectors(), W, H, GAP, HEADER);
    let (zoom, set_zoom) = signal(1.0_f64);

    let board = boxes
        .into_iter()
        .map(|b| {
            let name = b.name;
            let box_style = format!(
                "position:absolute; left:{:.1}px; top:{:.1}px; width:{:.1}px; height:{:.1}px;",
                b.rect.x,
                b.rect.y,
                (b.rect.w - 1.0).max(0.0),
                (b.rect.h - 1.0).max(0.0),
            );
            let label_style = format!(
                "position:absolute; left:{:.1}px; top:{:.1}px; max-width:{:.1}px;",
                b.rect.x + 6.0,
                b.rect.y + 3.0,
                (b.rect.w - 10.0).max(0.0),
            );
            let tiles = b.tiles.into_iter().map(|t| tile_view(t, tf_idx)).collect_view();
            view! {
                <div class="sector-box" style=box_style></div>
                <div class="sector-label" style=label_style>{name}</div>
                {tiles}
            }
        })
        .collect_view();

    view! {
        <div class="zoom-bar">
            <span>"放大"</span>
            <input
                type="range"
                min="1"
                max="4"
                step="0.1"
                prop:value=move || zoom.get().to_string()
                on:input=move |ev| {
                    if let Ok(z) = event_target_value(&ev).parse::<f64>() {
                        set_zoom.set(z);
                    }
                }
            />
            <span class="tf-label">{move || format!("{:.1}x", zoom.get())}</span>
        </div>
        <div class="viewport">
            // spacer reserves the scaled size so the viewport can scroll/pan
            <div
                class="zoom-space"
                style=move || format!("width:{}px; height:{}px;", W * zoom.get(), H * zoom.get())
            >
                <div
                    class="heatmap"
                    style=move || {
                        format!(
                            "position:absolute; left:0; top:0; width:{W}px; height:{H}px; \
                             transform:scale({}); transform-origin:top left;",
                            zoom.get(),
                        )
                    }
                >
                    {board}
                </div>
            </div>
        </div>
    }
}

/// One stock tile: fixed treemap geometry + reactive color. Hover glow + tooltip.
/// Very short tiles drop the %change and keep just the symbol.
fn tile_view(t: StockTile, tf_idx: ReadSignal<usize>) -> impl IntoView {
    let StockTile { symbol, market_cap, closes, rect } = t;
    let show_pct = rect.h >= 34.0;
    let pct = Memo::new(move |_| pct_change(&closes, Timeframe::ALL[tf_idx.get()].lookback_days()));
    let geom = format!(
        "position:absolute; left:{:.1}px; top:{:.1}px; width:{:.1}px; height:{:.1}px;",
        rect.x,
        rect.y,
        (rect.w - TILE_GAP).max(0.0),
        (rect.h - TILE_GAP).max(0.0),
    );
    view! {
        <div
            class="tile"
            title=move || {
                let p = pct.get().map(|p| format!("{p:+.2}%")).unwrap_or_else(|| "—".to_string());
                format!("{symbol}  ·  市值 ${market_cap:.0}B  ·  {p}")
            }
            style=move || {
                let bg = pct.get().map(color).unwrap_or_else(|| "#333".to_string());
                format!("{geom} background:{bg}")
            }
        >
            <span class="sym">{symbol}</span>
            {show_pct
                .then(|| {
                    view! {
                        <span class="pct">
                            {move || pct.get().map(|p| format!("{p:+.1}%")).unwrap_or_default()}
                        </span>
                    }
                })}
        </div>
    }
}
