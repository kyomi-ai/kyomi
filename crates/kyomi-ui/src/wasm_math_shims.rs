// SPDX-License-Identifier: AGPL-3.0-or-later

//! C math shims for `wasm32-unknown-unknown`.
//!
//! Rust's prebuilt stdlib on this target lowers `f{32,64}::acosh/asinh/atanh`
//! to extern C calls that the linker cannot resolve (the target ships no libm).
//! DataFusion's default math UDF set (`AcoshFunc`, `AsinhFunc`, `AtanhFunc`)
//! is alive in release builds via `SessionStateBuilder::with_default_features`,
//! so the references aren't dead-code-eliminated. Forward them to `libm`.

#[unsafe(no_mangle)]
pub extern "C" fn acosh(x: f64) -> f64 {
    libm::acosh(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn acoshf(x: f32) -> f32 {
    libm::acoshf(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn asinh(x: f64) -> f64 {
    libm::asinh(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn asinhf(x: f32) -> f32 {
    libm::asinhf(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn atanh(x: f64) -> f64 {
    libm::atanh(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn atanhf(x: f32) -> f32 {
    libm::atanhf(x)
}
