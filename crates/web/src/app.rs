//! Root component: holds the selected timeframe (index into Timeframe::ALL) as
//! reactive state, renders the slider + heatmap. Dragging the slider updates the
//! state -> the heatmap re-derives each tile's %change and recolors.

use crate::components::{heatmap::Heatmap, timeframe_slider::TimeframeSlider};
use leptos::prelude::*;

#[component]
pub fn App() -> impl IntoView {
    // default to the last timeframe (1Y) — most color to look at up front
    let (tf_idx, set_tf_idx) = signal(4usize);
    view! {
        <h1 class="title">"板块热力图"</h1>
        <TimeframeSlider idx=tf_idx set_idx=set_tf_idx />
        <Heatmap tf_idx=tf_idx />
    }
}
