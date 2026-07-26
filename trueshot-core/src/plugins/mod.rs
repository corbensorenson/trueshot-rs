use anyhow::{Context, Result};
use std::path::Path;
use wasmtime::*;

pub struct PluginEngine {
    engine: Engine,
    linker: Linker<ContextState>,
}

struct ContextState {
    logs: Vec<String>,
}

impl PluginEngine {
    pub fn new() -> Result<Self> {
        let engine = Engine::default();
        let mut linker = Linker::new(&engine);

        // Host Functions
        linker.func_wrap(
            "host",
            "log",
            |mut caller: Caller<'_, ContextState>, ptr: i32, len: i32| {
                let memory = match caller.get_export("memory") {
                    Some(Extern::Memory(mem)) => mem,
                    _ => {
                        tracing::warn!("Plugin log called without exported memory");
                        return;
                    }
                };
                if ptr < 0 || len < 0 {
                    tracing::warn!("Plugin log called with negative ptr/len");
                    return;
                }
                let ptr = ptr as usize;
                let len = len as usize;
                let mut buf = vec![0u8; len];
                if memory.read(&caller, ptr, &mut buf).is_err() {
                    tracing::warn!("Plugin log memory read failed");
                    return;
                }
                match String::from_utf8(buf) {
                    Ok(text) => {
                        caller.data_mut().logs.push(text.clone());
                        tracing::info!("Plugin: {}", text);
                    }
                    Err(_) => {
                        tracing::warn!("Plugin log contained invalid UTF-8");
                    }
                }
            },
        )?;

        Ok(Self { engine, linker })
    }

    pub fn process_image(&self, wasm_path: &Path, image_data: &[u8]) -> Result<Vec<u8>> {
        let module = Module::from_file(&self.engine, wasm_path)?;
        let mut store = Store::new(&self.engine, ContextState { logs: vec![] });

        let instance = self.linker.instantiate(&mut store, &module)?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .context("Plugin must export memory")?;

        // 1. Allocate memory in guest for input
        let alloc_fn = instance
            .get_typed_func::<i32, i32>(&mut store, "alloc")
            .context("Plugin must export alloc(size) -> ptr")?;
        let input_ptr = alloc_fn.call(&mut store, image_data.len() as i32)?;

        // 2. Write input image
        memory.write(&mut store, input_ptr as usize, image_data)?;

        // 3. Call process(ptr, len) -> output_ptr_len (packed u64 or just ptr, assume ptr)
        // For simplicity, let's say process returns a struct pointer or just modify in place?
        // Let's assume process(ptr, len) -> output_ptr
        // And guest exports "get_output_len"
        let process_fn = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, "process_image")
            .context("Plugin must export process_image(ptr, len) -> ptr")?;

        let output_ptr = process_fn.call(&mut store, (input_ptr, image_data.len() as i32))?;

        // 4. Get Output Length (Guest convention required)
        let get_len_fn = instance
            .get_typed_func::<i32, i32>(&mut store, "get_len")
            .context("Plugin must export get_len(ptr) -> len")?;
        let output_len = get_len_fn.call(&mut store, output_ptr)?;

        // 5. Read output
        let mut output_buffer = vec![0u8; output_len as usize];
        memory.read(&mut store, output_ptr as usize, &mut output_buffer)?;

        // 6. Deallocate (Guest responsible for cleaning up input/output if needed, or we just drop instance)

        Ok(output_buffer)
    }
}
