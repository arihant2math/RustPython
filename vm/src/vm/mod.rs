//! Implement virtual machine to run instructions.
//!
//! See also:
//!   <https://github.com/ProgVal/pythonvm-rust/blob/master/src/processor/mod.rs>

#[cfg(feature = "rustpython-compiler")]
mod compile;
mod context;
mod interpreter;
mod method;
mod setting;
pub mod thread;
mod vm_new;
mod vm_object;
mod vm_ops;

use crate::{
    builtins::{
        code::PyCode,
        pystr::IntoPyStrRef,
        tuple::{PyTuple, PyTupleTyped},
        PyBaseExceptionRef, PyDictRef, PyInt, PyList, PyModule, PyStrRef, PyTypeRef,
    },
    bytecode,
    codecs::CodecsRegistry,
    common::{ascii, hash::HashSecret, lock::PyMutex, rc::PyRc},
    convert::{ToPyObject, TryFromObject},
    frame::{ExecutionResult, Frame, FrameRef},
    frozen,
    function::{ArgMapping, FuncArgs},
    import,
    protocol::PyIterIter,
    scope::Scope,
    signal, stdlib, AsObject, PyObject, PyObjectRef, PyPayload, PyRef, PyResult,
};
use crossbeam_utils::atomic::AtomicCell;
use std::{
    borrow::Cow,
    cell::{Cell, Ref, RefCell},
    collections::{HashMap, HashSet},
};

pub use context::Context;
pub use interpreter::Interpreter;
pub(crate) use method::PyMethod;
pub use setting::Settings;

// Objects are live when they are on stack, or referenced by a name (for now)

/// Top level container of a python virtual machine. In theory you could
/// create more instances of this struct and have them operate fully isolated.
///
/// To construct this, please refer to the [`Interpreter`](Interpreter)
pub struct VirtualMachine {
    pub builtins: PyRef<PyModule>,
    pub sys_module: PyRef<PyModule>,
    pub ctx: PyRc<Context>,
    pub frames: RefCell<Vec<FrameRef>>,
    pub wasm_id: Option<String>,
    exceptions: RefCell<ExceptionStack>,
    pub import_func: PyObjectRef,
    pub profile_func: RefCell<PyObjectRef>,
    pub trace_func: RefCell<PyObjectRef>,
    pub use_tracing: Cell<bool>,
    pub recursion_limit: Cell<usize>,
    pub(crate) signal_handlers: Option<Box<RefCell<[Option<PyObjectRef>; signal::NSIG]>>>,
    pub(crate) signal_rx: Option<signal::UserSignalReceiver>,
    pub repr_guards: RefCell<HashSet<usize>>,
    pub state: PyRc<PyGlobalState>,
    pub initialized: bool,
    recursion_depth: Cell<usize>,
}

#[derive(Debug, Default)]
struct ExceptionStack {
    exc: Option<PyBaseExceptionRef>,
    prev: Option<Box<ExceptionStack>>,
}

pub struct PyGlobalState {
    pub settings: Settings,
    pub module_inits: stdlib::StdlibMap,
    pub frozen: HashMap<String, bytecode::FrozenModule, ahash::RandomState>,
    pub stacksize: AtomicCell<usize>,
    pub thread_count: AtomicCell<usize>,
    pub hash_secret: HashSecret,
    pub atexit_funcs: PyMutex<Vec<(PyObjectRef, FuncArgs)>>,
    pub codec_registry: CodecsRegistry,
}

impl VirtualMachine {
    /// Create a new `VirtualMachine` structure.
    fn new(settings: Settings) -> VirtualMachine {
        flame_guard!("new VirtualMachine");
        let ctx = Context::default();

        // make a new module without access to the vm; doesn't
        // set __spec__, __loader__, etc. attributes
        let new_module = || {
            PyRef::new_ref(
                PyModule {},
                ctx.types.module_type.clone(),
                Some(ctx.new_dict()),
            )
        };

        // Hard-core modules:
        let builtins = new_module();
        let sys_module = new_module();

        let import_func = ctx.none();
        let profile_func = RefCell::new(ctx.none());
        let trace_func = RefCell::new(ctx.none());
        // hack to get around const array repeat expressions, rust issue #79270
        const NONE: Option<PyObjectRef> = None;
        // putting it in a const optimizes better, prevents linear initialization of the array
        #[allow(clippy::declare_interior_mutable_const)]
        const SIGNAL_HANDLERS: RefCell<[Option<PyObjectRef>; signal::NSIG]> =
            RefCell::new([NONE; signal::NSIG]);
        let signal_handlers = Some(Box::new(SIGNAL_HANDLERS));

        let module_inits = stdlib::get_module_inits();

        let hash_secret = match settings.hash_seed {
            Some(seed) => HashSecret::new(seed),
            None => rand::random(),
        };

        let codec_registry = CodecsRegistry::new(&ctx);

        let mut vm = VirtualMachine {
            builtins,
            sys_module,
            ctx: PyRc::new(ctx),
            frames: RefCell::new(vec![]),
            wasm_id: None,
            exceptions: RefCell::default(),
            import_func,
            profile_func,
            trace_func,
            use_tracing: Cell::new(false),
            recursion_limit: Cell::new(if cfg!(debug_assertions) { 256 } else { 1000 }),
            signal_handlers,
            signal_rx: None,
            repr_guards: RefCell::default(),
            state: PyRc::new(PyGlobalState {
                settings,
                module_inits,
                frozen: HashMap::default(),
                stacksize: AtomicCell::new(0),
                thread_count: AtomicCell::new(0),
                hash_secret,
                atexit_funcs: PyMutex::default(),
                codec_registry,
            }),
            initialized: false,
            recursion_depth: Cell::new(0),
        };

        let frozen = frozen::get_module_inits().collect();
        PyRc::get_mut(&mut vm.state).unwrap().frozen = frozen;

        vm.builtins.init_module_dict(
            vm.ctx.new_str(ascii!("builtins")).into(),
            vm.ctx.none(),
            &vm,
        );
        vm.sys_module
            .init_module_dict(vm.ctx.new_str(ascii!("sys")).into(), vm.ctx.none(), &vm);

        vm
    }

    fn initialize(&mut self) {
        flame_guard!("init VirtualMachine");

        if self.initialized {
            panic!("Double Initialize Error");
        }

        stdlib::builtins::make_module(self, self.builtins.clone().into());
        stdlib::sys::init_module(self, self.sys_module.as_ref(), self.builtins.as_ref());

        let mut inner_init = || -> PyResult<()> {
            #[cfg(not(target_arch = "wasm32"))]
            import::import_builtin(self, "_signal")?;
            import::init_importlib(self, self.state.settings.allow_external_library)?;

            // set up the encodings search function
            self.import("encodings", None, 0).map_err(|import_err| {
                let err = self.new_runtime_error(
                    "Could not import encodings. Is your RUSTPYTHONPATH set? If you don't have \
                     access to a consistent external environment (e.g. if you're embedding \
                     rustpython in another application), try enabling the freeze-stdlib feature"
                        .to_owned(),
                );
                err.set_cause(Some(import_err));
                err
            })?;

            #[cfg(any(not(target_arch = "wasm32"), target_os = "wasi"))]
            {
                // this isn't fully compatible with CPython; it imports "io" and sets
                // builtins.open to io.OpenWrapper, but this is easier, since it doesn't
                // require the Python stdlib to be present
                let io = import::import_builtin(self, "_io")?;
                let set_stdio = |name, fd, mode: &str| {
                    let stdio = crate::stdlib::io::open(
                        self.ctx.new_int(fd).into(),
                        Some(mode),
                        Default::default(),
                        self,
                    )?;
                    self.sys_module.set_attr(
                        format!("__{}__", name), // e.g. __stdin__
                        stdio.clone(),
                        self,
                    )?;
                    self.sys_module.set_attr(name, stdio, self)?;
                    Ok(())
                };
                set_stdio("stdin", 0, "r")?;
                set_stdio("stdout", 1, "w")?;
                set_stdio("stderr", 2, "w")?;

                let io_open = io.get_attr("open", self)?;
                self.builtins.set_attr("open", io_open, self)?;
            }

            Ok(())
        };

        let res = inner_init();

        self.expect_pyresult(res, "initialization failed");

        self.initialized = true;
    }

    fn state_mut(&mut self) -> &mut PyGlobalState {
        PyRc::get_mut(&mut self.state)
            .expect("there should not be multiple threads while a user has a mut ref to a vm")
    }

    /// Can only be used in the initialization closure passed to [`Interpreter::with_init`]
    pub fn add_native_module<S>(&mut self, name: S, module: stdlib::StdlibInitFunc)
    where
        S: Into<Cow<'static, str>>,
    {
        self.state_mut().module_inits.insert(name.into(), module);
    }

    pub fn add_native_modules<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = (Cow<'static, str>, stdlib::StdlibInitFunc)>,
    {
        self.state_mut().module_inits.extend(iter);
    }

    /// Can only be used in the initialization closure passed to [`Interpreter::with_init`]
    pub fn add_frozen<I>(&mut self, frozen: I)
    where
        I: IntoIterator<Item = (String, bytecode::FrozenModule)>,
    {
        self.state_mut().frozen.extend(frozen);
    }

    /// Set the custom signal channel for the interpreter
    pub fn set_user_signal_channel(&mut self, signal_rx: signal::UserSignalReceiver) {
        self.signal_rx = Some(signal_rx);
    }

    pub fn run_code_obj(&self, code: PyRef<PyCode>, scope: Scope) -> PyResult {
        let frame = Frame::new(code, scope, self.builtins.dict(), &[], self).into_ref(self);
        self.run_frame(frame)
    }

    #[cold]
    pub fn run_unraisable(&self, e: PyBaseExceptionRef, msg: Option<String>, object: PyObjectRef) {
        let sys_module = self.import("sys", None, 0).unwrap();
        let unraisablehook = sys_module.get_attr("unraisablehook", self).unwrap();

        let exc_type = e.class().clone();
        let exc_traceback = e.traceback().to_pyobject(self); // TODO: actual traceback
        let exc_value = e.into();
        let args = stdlib::sys::UnraisableHookArgs {
            exc_type,
            exc_value,
            exc_traceback,
            err_msg: self.new_pyobj(msg),
            object,
        };
        if let Err(e) = self.invoke(&unraisablehook, (args,)) {
            println!("{}", e.as_object().repr(self).unwrap().as_str());
        }
    }

    #[inline(always)]
    pub fn run_frame(&self, frame: FrameRef) -> PyResult {
        match self.with_frame(frame, |f| f.run(self))? {
            ExecutionResult::Return(value) => Ok(value),
            _ => panic!("Got unexpected result from function"),
        }
    }

    pub fn current_recursion_depth(&self) -> usize {
        self.recursion_depth.get()
    }

    /// Used to run the body of a (possibly) recursive function. It will raise a
    /// RecursionError if recursive functions are nested far too many times,
    /// preventing a stack overflow.
    pub fn with_recursion<R, F: FnOnce() -> PyResult<R>>(&self, _where: &str, f: F) -> PyResult<R> {
        self.check_recursive_call(_where)?;
        self.recursion_depth.set(self.recursion_depth.get() + 1);
        let result = f();
        self.recursion_depth.set(self.recursion_depth.get() - 1);
        result
    }

    pub fn with_frame<R, F: FnOnce(FrameRef) -> PyResult<R>>(
        &self,
        frame: FrameRef,
        f: F,
    ) -> PyResult<R> {
        self.with_recursion("", || {
            self.frames.borrow_mut().push(frame.clone());
            let result = f(frame);
            // defer dec frame
            let _popped = self.frames.borrow_mut().pop();
            result
        })
    }

    // To be called right before raising the recursion depth.
    fn check_recursive_call(&self, _where: &str) -> PyResult<()> {
        if self.recursion_depth.get() >= self.recursion_limit.get() {
            Err(self.new_recursion_error(format!("maximum recursion depth exceeded {}", _where)))
        } else {
            Ok(())
        }
    }

    pub fn current_frame(&self) -> Option<Ref<FrameRef>> {
        let frames = self.frames.borrow();
        if frames.is_empty() {
            None
        } else {
            Some(Ref::map(self.frames.borrow(), |frames| {
                frames.last().unwrap()
            }))
        }
    }

    pub fn current_locals(&self) -> PyResult<ArgMapping> {
        self.current_frame()
            .expect("called current_locals but no frames on the stack")
            .locals(self)
    }

    pub fn current_globals(&self) -> Ref<PyDictRef> {
        let frame = self
            .current_frame()
            .expect("called current_globals but no frames on the stack");
        Ref::map(frame, |f| &f.globals)
    }

    pub fn try_class(&self, module: &str, class: &str) -> PyResult<PyTypeRef> {
        let class = self
            .import(module, None, 0)?
            .get_attr(class, self)?
            .downcast()
            .expect("not a class");
        Ok(class)
    }

    pub fn class(&self, module: &str, class: &str) -> PyTypeRef {
        let module = self
            .import(module, None, 0)
            .unwrap_or_else(|_| panic!("unable to import {}", module));
        let class = module
            .get_attr(class, self)
            .unwrap_or_else(|_| panic!("module {} has no class {}", module, class));
        class.downcast().expect("not a class")
    }

    #[inline]
    pub fn import(
        &self,
        module: impl IntoPyStrRef,
        from_list: Option<PyTupleTyped<PyStrRef>>,
        level: usize,
    ) -> PyResult {
        self._import_inner(module.into_pystr_ref(self), from_list, level)
    }

    fn _import_inner(
        &self,
        module: PyStrRef,
        from_list: Option<PyTupleTyped<PyStrRef>>,
        level: usize,
    ) -> PyResult {
        // if the import inputs seem weird, e.g a package import or something, rather than just
        // a straight `import ident`
        let weird = module.as_str().contains('.')
            || level != 0
            || from_list.as_ref().map_or(false, |x| !x.is_empty());

        let cached_module = if weird {
            None
        } else {
            let sys_modules = self.sys_module.get_attr("modules", self)?;
            sys_modules.get_item(&*module, self).ok()
        };

        match cached_module {
            Some(cached_module) => {
                if self.is_none(&cached_module) {
                    Err(self.new_import_error(
                        format!("import of {} halted; None in sys.modules", module),
                        module,
                    ))
                } else {
                    Ok(cached_module)
                }
            }
            None => {
                let import_func =
                    self.builtins
                        .clone()
                        .get_attr("__import__", self)
                        .map_err(|_| {
                            self.new_import_error("__import__ not found".to_owned(), module.clone())
                        })?;

                let (locals, globals) = if let Some(frame) = self.current_frame() {
                    (Some(frame.locals.clone()), Some(frame.globals.clone()))
                } else {
                    (None, None)
                };
                let from_list = match from_list {
                    Some(tup) => tup.to_pyobject(self),
                    None => self.new_tuple(()).into(),
                };
                self.invoke(&import_func, (module, globals, locals, from_list, level))
                    .map_err(|exc| import::remove_importlib_frames(self, &exc))
            }
        }
    }

    pub fn extract_elements_with<T, F>(&self, value: &PyObject, func: F) -> PyResult<Vec<T>>
    where
        F: Fn(PyObjectRef) -> PyResult<T>,
    {
        // Extract elements from item, if possible:
        let cls = value.class();
        let list_borrow;
        let slice = if cls.is(&self.ctx.types.tuple_type) {
            value.payload::<PyTuple>().unwrap().as_slice()
        } else if cls.is(&self.ctx.types.list_type) {
            list_borrow = value.payload::<PyList>().unwrap().borrow_vec();
            &list_borrow
        } else {
            return self.map_pyiter(value, func);
        };
        slice.iter().map(|obj| func(obj.clone())).collect()
    }

    pub fn map_iterable_object<F, R>(&self, obj: &PyObject, mut f: F) -> PyResult<PyResult<Vec<R>>>
    where
        F: FnMut(PyObjectRef) -> PyResult<R>,
    {
        match_class!(match obj {
            ref l @ PyList => {
                let mut i: usize = 0;
                let mut results = Vec::with_capacity(l.borrow_vec().len());
                loop {
                    let elem = {
                        let elements = &*l.borrow_vec();
                        if i >= elements.len() {
                            results.shrink_to_fit();
                            return Ok(Ok(results));
                        } else {
                            elements[i].clone()
                        }
                        // free the lock
                    };
                    match f(elem) {
                        Ok(result) => results.push(result),
                        Err(err) => return Ok(Err(err)),
                    }
                    i += 1;
                }
            }
            ref t @ PyTuple => Ok(t.iter().cloned().map(f).collect()),
            // TODO: put internal iterable type
            obj => {
                Ok(self.map_pyiter(obj, f))
            }
        })
    }

    fn map_pyiter<F, R>(&self, value: &PyObject, mut f: F) -> PyResult<Vec<R>>
    where
        F: FnMut(PyObjectRef) -> PyResult<R>,
    {
        let iter = value.to_owned().get_iter(self)?;
        let cap = match self.length_hint_opt(value.to_owned()) {
            Err(e) if e.class().is(&self.ctx.exceptions.runtime_error) => return Err(e),
            Ok(Some(value)) => Some(value),
            // Use a power of 2 as a default capacity.
            _ => None,
        };
        // TODO: fix extend to do this check (?), see test_extend in Lib/test/list_tests.py,
        // https://github.com/python/cpython/blob/v3.9.0/Objects/listobject.c#L922-L928
        if let Some(cap) = cap {
            if cap >= isize::max_value() as usize {
                return Ok(Vec::new());
            }
        }

        let mut results = PyIterIter::new(self, iter.as_ref(), cap)
            .map(|element| f(element?))
            .collect::<PyResult<Vec<_>>>()?;
        results.shrink_to_fit();
        Ok(results)
    }

    pub fn get_attribute_opt<T>(
        &self,
        obj: PyObjectRef,
        attr_name: T,
    ) -> PyResult<Option<PyObjectRef>>
    where
        T: IntoPyStrRef,
    {
        match obj.get_attr(attr_name, self) {
            Ok(attr) => Ok(Some(attr)),
            Err(e) if e.fast_isinstance(&self.ctx.exceptions.attribute_error) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn set_attribute_error_context(
        &self,
        exc: &PyBaseExceptionRef,
        obj: PyObjectRef,
        name: PyStrRef,
    ) {
        if exc.class().is(&self.ctx.exceptions.attribute_error) {
            let exc = exc.as_object();
            exc.set_attr("name", name, self).unwrap();
            exc.set_attr("obj", obj, self).unwrap();
        }
    }

    // get_method should be used for internal access to magic methods (by-passing
    // the full getattribute look-up.
    pub fn get_method_or_type_error<F>(
        &self,
        obj: PyObjectRef,
        method_name: &str,
        err_msg: F,
    ) -> PyResult
    where
        F: FnOnce() -> String,
    {
        let method = obj
            .class()
            .get_attr(method_name)
            .ok_or_else(|| self.new_type_error(err_msg()))?;
        self.call_if_get_descriptor(method, obj)
    }

    // TODO: remove + transfer over to get_special_method
    pub(crate) fn get_method(&self, obj: PyObjectRef, method_name: &str) -> Option<PyResult> {
        let method = obj.get_class_attr(method_name)?;
        Some(self.call_if_get_descriptor(method, obj))
    }

    pub fn is_callable(&self, obj: &PyObject) -> bool {
        obj.class()
            .mro_find_map(|cls| cls.slots.call.load())
            .is_some()
    }

    #[inline]
    /// Checks for triggered signals and calls the appropriate handlers. A no-op on
    /// platforms where signals are not supported.
    pub fn check_signals(&self) -> PyResult<()> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::signal::check_signals(self)
        }
        #[cfg(target_arch = "wasm32")]
        {
            Ok(())
        }
    }

    pub(crate) fn push_exception(&self, exc: Option<PyBaseExceptionRef>) {
        let mut excs = self.exceptions.borrow_mut();
        let prev = std::mem::take(&mut *excs);
        excs.prev = Some(Box::new(prev));
        excs.exc = exc
    }

    pub(crate) fn pop_exception(&self) -> Option<PyBaseExceptionRef> {
        let mut excs = self.exceptions.borrow_mut();
        let cur = std::mem::take(&mut *excs);
        *excs = *cur.prev.expect("pop_exception() without nested exc stack");
        cur.exc
    }

    pub(crate) fn take_exception(&self) -> Option<PyBaseExceptionRef> {
        self.exceptions.borrow_mut().exc.take()
    }

    pub(crate) fn current_exception(&self) -> Option<PyBaseExceptionRef> {
        self.exceptions.borrow().exc.clone()
    }

    pub(crate) fn set_exception(&self, exc: Option<PyBaseExceptionRef>) {
        // don't be holding the refcell guard while __del__ is called
        let prev = std::mem::replace(&mut self.exceptions.borrow_mut().exc, exc);
        drop(prev);
    }

    pub(crate) fn contextualize_exception(&self, exception: &PyBaseExceptionRef) {
        if let Some(context_exc) = self.topmost_exception() {
            if !context_exc.is(exception) {
                let mut o = context_exc.clone();
                while let Some(context) = o.context() {
                    if context.is(exception) {
                        o.set_context(None);
                        break;
                    }
                    o = context;
                }
                exception.set_context(Some(context_exc))
            }
        }
    }

    pub(crate) fn topmost_exception(&self) -> Option<PyBaseExceptionRef> {
        let excs = self.exceptions.borrow();
        let mut cur = &*excs;
        loop {
            if let Some(exc) = &cur.exc {
                return Some(exc.clone());
            }
            cur = cur.prev.as_deref()?;
        }
    }

    pub fn handle_exit_exception(&self, exc: PyBaseExceptionRef) -> i32 {
        if exc.fast_isinstance(&self.ctx.exceptions.system_exit) {
            let args = exc.args();
            let msg = match args.as_slice() {
                [] => return 0,
                [arg] => match_class!(match arg {
                    ref i @ PyInt => {
                        use num_traits::cast::ToPrimitive;
                        return i.as_bigint().to_i32().unwrap_or(0);
                    }
                    arg => {
                        if self.is_none(arg) {
                            return 0;
                        } else {
                            arg.str(self).ok()
                        }
                    }
                }),
                _ => args.as_object().repr(self).ok(),
            };
            if let Some(msg) = msg {
                let stderr = stdlib::sys::PyStderr(self);
                writeln!(stderr, "{}", msg);
            }
            1
        } else {
            self.print_exception(exc);
            1
        }
    }

    #[doc(hidden)]
    pub fn __module_set_attr(
        &self,
        module: &PyObject,
        attr_name: impl IntoPyStrRef,
        attr_value: impl Into<PyObjectRef>,
    ) -> PyResult<()> {
        let val = attr_value.into();
        module.generic_setattr(attr_name.into_pystr_ref(self), Some(val), self)
    }

    pub fn insert_sys_path(&self, obj: PyObjectRef) -> PyResult<()> {
        let sys_path = self.sys_module.get_attr("path", self).unwrap();
        self.call_method(&sys_path, "insert", (0, obj))?;
        Ok(())
    }

    pub fn run_script(&self, scope: Scope, path: &str) -> PyResult<()> {
        if get_importer(path, self)?.is_some() {
            self.insert_sys_path(self.new_pyobj(path))?;
            let runpy = self.import("runpy", None, 0)?;
            let run_module_as_main = runpy.get_attr("_run_module_as_main", self)?;
            self.invoke(&run_module_as_main, (self.ctx.new_str("__main__"), false))?;
            return Ok(());
        }

        let dir = std::path::Path::new(path)
            .parent()
            .unwrap()
            .to_str()
            .unwrap();
        self.insert_sys_path(self.new_pyobj(dir))?;

        match std::fs::read_to_string(path) {
            Ok(source) => {
                self.run_code_string(scope, &source, path.to_owned())?;
            }
            Err(err) => {
                error!("Failed reading file '{}': {}", path, err);
                std::process::exit(1);
            }
        }
        Ok(())
    }

    pub fn run_code_string(&self, scope: Scope, source: &str, source_path: String) -> PyResult {
        let code_obj = self
            .compile(source, crate::compile::Mode::Exec, source_path.clone())
            .map_err(|err| self.new_syntax_error(&err))?;
        // trace!("Code object: {:?}", code_obj.borrow());
        scope
            .globals
            .set_item("__file__", self.new_pyobj(source_path), self)?;
        self.run_code_obj(code_obj, scope)
    }

    pub fn run_module(&self, module: &str) -> PyResult<()> {
        let runpy = self.import("runpy", None, 0)?;
        let run_module_as_main = runpy.get_attr("_run_module_as_main", self)?;
        self.invoke(&run_module_as_main, (module,))?;
        Ok(())
    }
}

fn get_importer(path: &str, vm: &VirtualMachine) -> PyResult<Option<PyObjectRef>> {
    let path_importer_cache = vm.sys_module.get_attr("path_importer_cache", vm)?;
    let path_importer_cache = PyDictRef::try_from_object(vm, path_importer_cache)?;
    if let Some(importer) = path_importer_cache.get_item_opt(path, vm)? {
        return Ok(Some(importer));
    }
    let path = vm.ctx.new_str(path);
    let path_hooks = vm.sys_module.get_attr("path_hooks", vm)?;
    let mut importer = None;
    let path_hooks: Vec<PyObjectRef> = path_hooks.try_into_value(vm)?;
    for path_hook in path_hooks {
        match vm.invoke(&path_hook, (path.clone(),)) {
            Ok(imp) => {
                importer = Some(imp);
                break;
            }
            Err(e) if e.fast_isinstance(&vm.ctx.exceptions.import_error) => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(if let Some(imp) = importer {
        let imp = path_importer_cache.get_or_insert(vm, path.into(), || imp.clone())?;
        Some(imp)
    } else {
        None
    })
}