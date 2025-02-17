#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub fn main() -> std::process::ExitCode {
    rustpython::run(|_vm| {})
}
