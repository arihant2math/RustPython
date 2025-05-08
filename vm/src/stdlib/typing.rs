pub(crate) use _typing::make_module;

#[pymodule]
pub(crate) mod _typing {
    use crate::{
        Py, PyObjectRef, PyPayload, PyResult, VirtualMachine,
        builtins::{PyGenericAlias, PyTupleRef, PyTypeRef, pystr::AsPyStr},
        convert::ToPyResult,
        function::{FuncArgs, IntoFuncArgs},
        types::{Constructor, Representable}
    };

    pub(crate) fn _call_typing_func_object<'a>(
        _vm: &VirtualMachine,
        _func_name: impl AsPyStr<'a>,
        _args: impl IntoFuncArgs,
    ) -> PyResult {
        todo!("does this work????");
        // let module = vm.import("typing", 0)?;
        // let module = vm.import("_pycodecs", None, 0)?;
        // let func = module.get_attr(func_name, vm)?;
        // func.call(args, vm)
    }

    #[pyfunction]
    pub(crate) fn _idfunc(args: FuncArgs, _vm: &VirtualMachine) -> PyObjectRef {
        args.args[0].clone()
    }

    #[pyattr]
    #[pyclass(name = "TypeVar")]
    #[derive(Debug, PyPayload)]
    #[allow(dead_code)]
    pub(crate) struct TypeVar {
        name: PyObjectRef, // TODO PyStrRef?
        bound: parking_lot::Mutex<PyObjectRef>,
        evaluate_bound: PyObjectRef,
        constraints: parking_lot::Mutex<PyObjectRef>,
        evaluate_constraints: PyObjectRef,
        covariant: bool,
        contravariant: bool,
        infer_variance: bool,
    }

    impl Representable for TypeVar {
        #[inline]
        fn repr_str(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<String> {
            if zelf.infer_variance {
                return zelf.name.str(vm).map(|s| s.to_string());
            }
            let variance = if zelf.covariant {
                '+'
            } else if zelf.contravariant {
                '-'
            } else {
                '~'
            };
            let name = zelf.name.str(vm)?;
            let name = name.to_string();
            Ok(format!("{}{}", variance, name))
        }
    }

    #[pyclass(flags(BASETYPE), with(Representable))]
    impl TypeVar {
        pub(crate) fn _bound(&self, vm: &VirtualMachine) -> PyResult {
            let mut bound = self.bound.lock();
            if !vm.is_none(&bound) {
                return Ok(bound.clone());
            }
            if !vm.is_none(&self.evaluate_bound) {
                *bound = self.evaluate_bound.call((), vm)?;
                Ok(bound.clone())
            } else {
                Ok(vm.ctx.none())
            }
        }

        #[pygetset(magic)]
        fn name(&self) -> PyObjectRef {
            self.name.clone()
        }

        #[pygetset(magic)]
        fn covariant(&self) -> bool {
            self.covariant
        }

        #[pygetset(magic)]
        fn contravariant(&self) -> bool {
            self.contravariant
        }

        #[pygetset(magic)]
        fn infer_variance(&self) -> bool {
            self.infer_variance
        }

        #[pymethod(magic)]
        fn mro_entries(&self, vm: &VirtualMachine) -> PyResult {
            Err(vm.new_type_error("Cannot subclass an instance of TypeVar".to_string()))
        }
    }

    pub(crate) fn make_typevar(
        vm: &VirtualMachine,
        name: PyObjectRef,
        evaluate_bound: PyObjectRef,
        evaluate_constraints: PyObjectRef,
    ) -> TypeVar {
        TypeVar {
            name,
            bound: parking_lot::Mutex::new(vm.ctx.none()),
            evaluate_bound,
            constraints: parking_lot::Mutex::new(vm.ctx.none()),
            evaluate_constraints,
            covariant: false,
            contravariant: false,
            infer_variance: true,
        }
    }

    #[pyattr]
    #[pyclass(name = "ParamSpec")]
    #[derive(Debug, PyPayload)]
    #[allow(dead_code)]
    pub(crate) struct ParamSpec {
        name: PyObjectRef,
        bound: Option<PyObjectRef>,
        default_value: Option<PyObjectRef>,
        evaluate_default: Option<PyObjectRef>,
        covariant: bool,
        contravariant: bool,
        infer_variance: bool,
    }

    #[derive(FromArgs, Debug)]
    pub(crate) struct ParamSpecConstructorArgs {
        #[pyarg(positional)]
        name: PyObjectRef,
        #[pyarg(positional, default = None)]
        bound: Option<PyObjectRef>,
        // TODO: Default is actually _Py_NoDefaultStruct
        #[pyarg(positional, default = None)]
        default_value: Option<PyObjectRef>,
        #[pyarg(positional, default = false)]
        covariant: bool,
        #[pyarg(positional, default = false)]
        contravariant: bool,
        #[pyarg(positional, default = false)]
        infer_variance: bool,
    }

    impl Constructor for ParamSpec {
        type Args = ParamSpecConstructorArgs;

        fn py_new(_cls: PyTypeRef, args: Self::Args, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            if vm.is_none(&args.name) {
                return Err(vm.new_type_error("ParamSpec name cannot be None".to_string()));
            }
            let paramspec = ParamSpec {
                name: args.name,
                bound: args.bound,
                default_value: args.default_value,
                evaluate_default: None,
                covariant: args.covariant,
                contravariant: args.contravariant,
                infer_variance: args.infer_variance,
            };
            paramspec.to_pyresult(vm)
        }
    }

    #[pyclass(flags(BASETYPE), with(Constructor))]
    impl ParamSpec {
        #[pygetset(magic)]
        fn name(&self) -> PyObjectRef {
            self.name.clone()
        }

        #[pygetset(magic)]
        fn bound(&self, vm: &VirtualMachine) -> PyObjectRef {
            if let Some(bound) = self.bound.clone() {
                return bound;
            }
            vm.ctx.none()
        }

        #[pygetset(magic)]
        fn covariant(&self) -> bool {
            self.covariant
        }

        #[pygetset(magic)]
        fn contravariant(&self) -> bool {
            self.contravariant
        }

        #[pygetset(magic)]
        fn infer_variance(&self) -> bool {
            self.infer_variance
        }

        #[pygetset(magic)]
        fn default(&self, vm: &VirtualMachine) -> PyResult {
            if let Some(default_value) = self.default_value.clone() {
                return Ok(default_value);
            }
            // handle evaluate_default
            if let Some(evaluate_default) = self.evaluate_default.clone() {
                let default_value = vm.call_method(evaluate_default.as_ref(), "__call__", ())?;
                return Ok(default_value);
            }
            // TODO: this isn't up to spec
            Ok(vm.ctx.none())
        }

        #[pygetset]
        fn evaluate_default(&self, vm: &VirtualMachine) -> PyObjectRef {
            if let Some(evaluate_default) = self.evaluate_default.clone() {
                return evaluate_default;
            }
            // TODO: default_value case
            vm.ctx.none()
        }

        #[pymethod(magic)]
        fn reduce(&self) -> PyResult {
            Ok(self.name.clone())
        }

        #[pymethod]
        fn has_default(&self) -> PyResult<bool> {
            // TODO: fix
            Ok(self.evaluate_default.is_some() || self.default_value.is_some())
        }

        #[pymethod(magic)]
        fn mro_entries(&self, vm: &VirtualMachine) -> PyResult {
            Err(vm.new_type_error("Cannot subclass an instance of ParamSpec".to_string()))
        }
    }

    pub(crate) fn make_paramspec(name: PyObjectRef) -> ParamSpec {
        ParamSpec {
            name,
            bound: None,
            default_value: None,
            evaluate_default: None,
            covariant: false,
            contravariant: false,
            infer_variance: false,
        }
    }

    #[pyattr]
    #[pyclass(module = false, name = "NoDefault")]
    #[derive(Debug, PyPayload)]
    #[allow(dead_code)]
    pub(crate) struct NoDefault {
        name: PyObjectRef,
    }

    impl Constructor for NoDefault {
        type Args = FuncArgs;

        fn py_new(_cls: PyTypeRef, args: Self::Args, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            if args.args.len() != 0 || args.kwargs.len() != 0 {
                return Err(vm.new_type_error("NoDefault takes no arguments".to_string()));
            }
            let no_default = NoDefault { name: vm.ctx.none() };
            no_default.to_pyresult(vm)
        }
    }

    impl Representable for NoDefault {
        fn repr_str(_zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            Ok("typing.NoDefault".to_string())
        }
    }

    #[pyclass(flags(BASETYPE), with(Constructor, Representable))]
    impl NoDefault {
        #[pymethod]
        fn reduce(&self) -> String {
            "NoDefault".to_string()
        }
    }

    #[pyattr]
    #[pyclass(name = "TypeVarTuple")]
    #[derive(Debug, PyPayload)]
    #[allow(dead_code)]
    pub(crate) struct TypeVarTuple {
        name: PyObjectRef,
    }
    #[pyclass(flags(BASETYPE))]
    impl TypeVarTuple {}

    pub(crate) fn make_typevartuple(name: PyObjectRef) -> TypeVarTuple {
        TypeVarTuple { name }
    }

    #[pyattr]
    #[pyclass(name = "ParamSpecArgs")]
    #[derive(Debug, PyPayload)]
    #[allow(dead_code)]
    pub(crate) struct ParamSpecArgs {}
    #[pyclass(flags(BASETYPE))]
    impl ParamSpecArgs {}

    #[pyattr]
    #[pyclass(name = "ParamSpecKwargs")]
    #[derive(Debug, PyPayload)]
    #[allow(dead_code)]
    pub(crate) struct ParamSpecKwargs {}
    #[pyclass(flags(BASETYPE))]
    impl ParamSpecKwargs {}

    #[pyattr]
    #[pyclass(name)]
    #[derive(Debug, PyPayload)]
    #[allow(dead_code)]
    pub(crate) struct TypeAliasType {
        name: PyObjectRef, // TODO PyStrRef?
        type_params: PyTupleRef,
        value: PyObjectRef,
        // compute_value: PyObjectRef,
        // module: PyObjectRef,
    }
    #[pyclass(flags(BASETYPE))]
    impl TypeAliasType {
        pub fn new(
            name: PyObjectRef,
            type_params: PyTupleRef,
            value: PyObjectRef,
        ) -> TypeAliasType {
            TypeAliasType {
                name,
                type_params,
                value,
            }
        }
    }

    #[pyattr]
    #[pyclass(name)]
    #[derive(Debug, PyPayload)]
    #[allow(dead_code)]
    pub(crate) struct Generic {}

    // #[pyclass(with(AsMapping), flags(BASETYPE))]
    #[pyclass(flags(BASETYPE))]
    impl Generic {
        #[pyclassmethod(magic)]
        fn class_getitem(cls: PyTypeRef, args: PyObjectRef, vm: &VirtualMachine) -> PyGenericAlias {
            PyGenericAlias::new(cls, args, vm)
        }
    }

    // impl AsMapping for Generic {
    //     fn as_mapping() -> &'static PyMappingMethods {
    //         static AS_MAPPING: Lazy<PyMappingMethods> = Lazy::new(|| PyMappingMethods {
    //             subscript: atomic_func!(|mapping, needle, vm| {
    //                 call_typing_func_object(vm, "_GenericAlias", (mapping.obj, needle))
    //             }),
    //             ..PyMappingMethods::NOT_IMPLEMENTED
    //         });
    //         &AS_MAPPING
    //     }
    // }
}
