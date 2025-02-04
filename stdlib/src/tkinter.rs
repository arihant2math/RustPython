use rustpython_vm::builtins::PyModule;
use rustpython_vm::{PyRef, VirtualMachine};

pub(crate) fn make_module(vm: &VirtualMachine) -> PyRef<PyModule> {
    let _ = vm.import("_hashlib", 0);
    _tkinter::make_module(vm)
}

#[pymodule]
mod _tkinter {
    use rustpython_vm::builtins::PyTypeRef;
    use rustpython_vm::VirtualMachine;
    // TODO: TK and TCL versions should not be hard coded
    #[pyattr]
    pub const TK_VERSION: &str = "8.6";
    #[pyattr]
    pub const TCL_VERSION: &str = "8.6";

    // All this should also not be hardcoded (these have been retrieved via cpython 3.13 on x86-64 windows)
    #[pyattr]
    pub const READABLE: i32 = 2;
    #[pyattr]
    pub const WRITABLE: i32 = 4;
    #[pyattr]
    pub const EXCEPTION: i32 = 8;

    #[pyattr(name = "TclError", once)]
    fn error(vm: &VirtualMachine) -> PyTypeRef {
        vm.ctx.new_exception_type(
            "_tkinter",
            "TclError",
            Some(vec![vm.ctx.exceptions.exception_type.to_owned()]),
        )
    }

    struct TkAppObject {

    }

    #[derive(FromArgs)]
    struct CreateArgs {
        #[pyarg(any, default = "")]
        screen_name: String,
        base_name: String,
        class_name: String,
        interactive: bool,
        want_objects: i32,
        want_tk: bool,
        sync: bool,
        tk_use: String,
    }

    #[pyfunction]
    fn create(args: CreateArgs, vm: &VirtualMachine) {

    }
}
