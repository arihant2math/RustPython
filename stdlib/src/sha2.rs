pub(crate) use _sha2::make_module;

#[pymodule]
mod _sha2 {
    use rustpython_vm::PyResult;

    #[pyfunction]
    fn sha224() -> PyResult<()> {
        Ok(())
    }
}
