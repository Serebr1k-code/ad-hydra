use crate::{DecodeError, Direction, ParsedMessage};
use hydra_protocol_sdk::{DecodeInput, DecodeOutput, MAX_INPUT, MAX_OUTPUT};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};
use wasmi::{Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};
fn error(e: impl std::fmt::Display) -> DecodeError {
    DecodeError(e.to_string())
}
pub struct PluginVersion {
    pub sha256: [u8; 32],
    pub metadata: Option<hydra_protocol_sdk::ParserMetadata>,
    engine: Engine,
    module: Module,
}
impl PluginVersion {
    pub fn load(bytes: &[u8], expected: [u8; 32]) -> Result<Arc<Self>, DecodeError> {
        if bytes.len() > 4 * 1024 * 1024 || <[u8; 32]>::from(Sha256::digest(bytes)) != expected {
            return Err(error("module size/checksum"));
        }
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes).map_err(error)?;
        if module.imports().next().is_some() {
            return Err(error("imports forbidden, including WASI"));
        }
        let version = Arc::new(Self {
            sha256: expected,
            metadata: None,
            engine,
            module,
        });
        let metadata = WasmSession::new(version.clone())?.metadata()?;
        let mut version = Arc::try_unwrap(version)
            .map_err(|_| error("unexpected retained module during validation"))?;
        version.metadata = metadata;
        Ok(Arc::new(version))
    }
}
#[derive(Default)]
pub struct PluginRegistry {
    versions: BTreeMap<String, Arc<PluginVersion>>,
    retained: Vec<Weak<PluginVersion>>,
}
impl PluginRegistry {
    pub fn activate(
        &mut self,
        profile: String,
        version: Arc<PluginVersion>,
    ) -> Result<(), DecodeError> {
        if profile.len() > 128
            || (!self.versions.contains_key(&profile) && self.versions.len() >= 16)
        {
            return Err(error("registry limit"));
        }
        // Live retired versions count too; reject repeated upgrades while old flows retain them.
        self.retained.retain(|v| v.strong_count() > 0);
        if !self
            .retained
            .iter()
            .any(|v| v.ptr_eq(&Arc::downgrade(&version)))
        {
            if self.retained.len() >= 16 {
                return Err(error("live version limit"));
            }
            self.retained.push(Arc::downgrade(&version));
        }
        self.versions.insert(profile, version);
        Ok(())
    }
    pub fn get(&self, profile: &str) -> Option<Arc<PluginVersion>> {
        self.versions.get(profile).cloned()
    }
}
pub struct WasmSession {
    version: Arc<PluginVersion>,
    store: Store<StoreLimits>,
    instance: wasmi::Instance,
    state: [Vec<u8>; 2],
    failed: bool,
}
impl WasmSession {
    pub fn new(version: Arc<PluginVersion>) -> Result<Self, DecodeError> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(4 * 1024 * 1024)
            .memories(1)
            .tables(1)
            .table_elements(1024)
            .instances(1)
            .build();
        let mut store = Store::new(&version.engine, limits);
        store.limiter(|l| l);
        store.set_fuel(200_000).map_err(error)?;
        let instance = Linker::<StoreLimits>::new(&version.engine)
            .instantiate(&mut store, &version.module)
            .map_err(error)?
            .start(&mut store)
            .map_err(error)?;
        let abi = instance
            .get_typed_func::<(), i32>(&store, "hydra_abi_version")
            .map_err(error)?
            .call(&mut store, ())
            .map_err(error)?;
        if abi != 1 {
            return Err(error("ABI version"));
        }
        instance
            .get_memory(&store, "memory")
            .ok_or_else(|| error("missing memory"))?;
        instance
            .get_typed_func::<i32, i32>(&store, "hydra_alloc")
            .map_err(error)?;
        instance
            .get_typed_func::<(i32, i32, i32, i32), ()>(&store, "hydra_dealloc")
            .map_err(error)?;
        instance
            .get_typed_func::<(i32, i32), i64>(&store, "hydra_decode")
            .map_err(error)?;
        Ok(Self {
            version,
            store,
            instance,
            state: [vec![], vec![]],
            failed: false,
        })
    }
    /// Optional ABI v1 extension: hydra_metadata() -> packed (pointer << 32 | length).
    /// The returned immutable JSON remains module-owned; capped at 8192 bytes.
    pub fn metadata(&mut self) -> Result<Option<hydra_protocol_sdk::ParserMetadata>, DecodeError> {
        if self
            .instance
            .get_export(&self.store, "hydra_metadata")
            .is_none()
        {
            return Ok(None);
        }
        self.store.set_fuel(200_000).map_err(error)?;
        let packed = self
            .instance
            .get_typed_func::<(), i64>(&self.store, "hydra_metadata")
            .map_err(error)?
            .call(&mut self.store, ())
            .map_err(error)? as u64;
        let len = packed as u32 as usize;
        if len > 8192 {
            return Err(error("metadata size limit"));
        }
        let mut bytes = vec![0; len];
        self.instance
            .get_memory(&self.store, "memory")
            .ok_or_else(|| error("memory"))?
            .read(&self.store, (packed >> 32) as usize, &mut bytes)
            .map_err(error)?;
        let metadata: hydra_protocol_sdk::ParserMetadata =
            serde_json::from_slice(&bytes).map_err(error)?;
        if !metadata.validate() {
            return Err(error("invalid metadata schema"));
        }
        Ok(Some(metadata))
    }
    pub fn version(&self) -> &Arc<PluginVersion> {
        &self.version
    }
    pub fn feed(
        &mut self,
        direction: Direction,
        bytes: &[u8],
    ) -> Result<Vec<ParsedMessage>, DecodeError> {
        if self.failed {
            return Err(error("decoder disabled"));
        }
        let result = self.decode(direction, bytes);
        if result.is_err() {
            self.failed = true;
            self.state = [vec![], vec![]];
        }
        result
    }
    fn decode(
        &mut self,
        direction: Direction,
        bytes: &[u8],
    ) -> Result<Vec<ParsedMessage>, DecodeError> {
        if bytes.len() > MAX_INPUT {
            return Err(error("input limit"));
        }
        let index = if direction == Direction::ClientToServer {
            0
        } else {
            1
        };
        let input = serde_json::to_vec(&DecodeInput {
            direction,
            bytes: bytes.to_vec(),
            state: self.state[index].clone(),
        })
        .map_err(error)?;
        if input.len() > 512 * 1024 {
            return Err(error("encoded input limit"));
        }
        self.store.set_fuel(200_000).map_err(error)?;
        let alloc = self
            .instance
            .get_typed_func::<i32, i32>(&self.store, "hydra_alloc")
            .map_err(error)?;
        let ptr = alloc
            .call(&mut self.store, input.len() as i32)
            .map_err(error)?;
        let memory = self
            .instance
            .get_memory(&self.store, "memory")
            .ok_or_else(|| error("memory"))?;
        memory
            .write(&mut self.store, ptr as u32 as usize, &input)
            .map_err(error)?;
        let packed = self
            .instance
            .get_typed_func::<(i32, i32), i64>(&self.store, "hydra_decode")
            .map_err(error)?
            .call(&mut self.store, (ptr, input.len() as i32))
            .map_err(error)? as u64;
        let out_ptr = (packed >> 32) as u32;
        let out_len = packed as u32;
        if out_len as usize > MAX_OUTPUT {
            return Err(error("output limit"));
        }
        let mut output = vec![0; out_len as usize];
        memory
            .read(&self.store, out_ptr as usize, &mut output)
            .map_err(error)?;
        self.instance
            .get_typed_func::<(i32, i32, i32, i32), ()>(&self.store, "hydra_dealloc")
            .map_err(error)?
            .call(
                &mut self.store,
                (ptr, input.len() as i32, out_ptr as i32, out_len as i32),
            )
            .map_err(error)?;
        let output: DecodeOutput = serde_json::from_slice(&output).map_err(error)?;
        if !output.validate() || output.messages.iter().any(|m| m.direction != direction) {
            return Err(error("invalid bounded output"));
        }
        self.state[index] = output.state;
        Ok(output.messages)
    }
}
