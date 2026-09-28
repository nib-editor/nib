//! SDK for writing nib plugins in Rust.
//!
//! Implement [`exports::nib::plugin::guest::Guest`] and register it with
//! [`export!`], then build for `wasm32-wasip2`.

wit_bindgen::generate!({
    path: "../../api/wit",
    world: "plugin",
    pub_export_macro: true,
    export_macro_name: "export",
    default_bindings_module: "nib_plugin",
    // Plugins compare keys and other values, so make that possible.
    additional_derives: [PartialEq, Eq, Hash],
    // These hold resources, which cannot be compared.
    additional_derives_ignore: ["buffer-change", "syntax-update", "event"],
});
