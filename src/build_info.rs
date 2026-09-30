//! Baked-in identity: the working source fingerprint distinguishes dirty builds.
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (revision ",
    env!("REC_BUILD_REVISION"), "; source ", env!("REC_BUILD_SOURCE"), ")");
