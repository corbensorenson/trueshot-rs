use anyhow::Result;
use wasmtime::*;

/// Plugin System (WASM)
/// Allows loading external .wasm modules to extend functionality via a safe sandbox.

pub struct PluginEngine {
    engine: Engine,
    linker: Linker<()>,
    store: Store<()>,
}

impl PluginEngine {
    pub fn new() -> Result<Self> {
        let config = Config::new();
        // Enable WASI if needed in future
        let engine = Engine::new(&config)?;
        let linker = Linker::new(&engine);
        let store = Store::new(&engine, ());

        Ok(Self {
            engine,
            linker,
            store,
        })
    }

    pub fn load_plugin(&mut self, path: &std::path::Path) -> Result<PluginInstance> {
        let module = Module::from_file(&self.engine, path)?;
        let instance = self.linker.instantiate(&mut self.store, &module)?;

        // Check for required exports
        let init_func = instance.get_typed_func::<(), ()>(&mut self.store, "init")?;

        Ok(PluginInstance {
            instance,
            init_func,
        })
    }

    pub fn run_plugin(&mut self, plugin: &PluginInstance) -> Result<()> {
        plugin.init_func.call(&mut self.store, ())?;
        Ok(())
    }
}

pub struct PluginInstance {
    instance: Instance,
    init_func: TypedFunc<(), ()>,
}
