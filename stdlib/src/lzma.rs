pub(crate) use _lzma::make_module;

#[pymodule(name = "_lzma")]
mod _lzma {
    #[pyfunction]
    fn is_check_supported(check_id: i32) -> bool {
        unsafe {
            lzma_sys::lzma_check_is_supported(check_id as _) != 0
        }
    }
}