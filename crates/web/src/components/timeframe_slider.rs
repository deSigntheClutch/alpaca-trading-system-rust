//! <TimeframeSlider> — the draggable bar: a native <input type="range"> over
//! Timeframe::ALL. Dragging fires `input` on every step -> set_idx -> heatmap
//! recolors live. No JS; Leptos wires the event.

use alpaca_core::data::Timeframe;
use leptos::prelude::*;

#[component]
pub fn TimeframeSlider(idx: ReadSignal<usize>, set_idx: WriteSignal<usize>) -> impl IntoView {
    let last = Timeframe::ALL.len() - 1; // max slider index
    view! {
        <div class="slider">
            <input
                type="range"
                min="0"
                max=last
                step="1"
                prop:value=move || idx.get().to_string()
                on:input=move |ev| {
                    if let Ok(v) = event_target_value(&ev).parse::<usize>() {
                        set_idx.set(v);
                    }
                }
            />
            <span class="tf-label">{move || Timeframe::ALL[idx.get()].label()}</span>
        </div>
    }
}
