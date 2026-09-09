//! wasm entry — trunk runs this. Set up readable panics, then mount the app.

mod app;
mod components;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
