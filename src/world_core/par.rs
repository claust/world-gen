#[cfg(not(target_arch = "wasm32"))]
macro_rules! maybe_par_iter {
    ($range:expr) => {
        $range.into_par_iter()
    };
}

#[cfg(target_arch = "wasm32")]
macro_rules! maybe_par_iter {
    ($range:expr) => {
        $range.into_iter()
    };
}
