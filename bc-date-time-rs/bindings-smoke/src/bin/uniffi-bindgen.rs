//! The binding generator, pinned by this crate's lockfile rather than by
//! whatever `uniffi-bindgen` happens to be on the developer's PATH.
fn main() {
    uniffi::uniffi_bindgen_main();
}
