use std::{env, path::PathBuf};

use schema_rust::{
    MetaListenerTier, NexusDaemonShape, SocketModeBits, WorkingListenerTier,
    build::{GenerationDriver, GenerationPlan, ModuleEmission},
};

const META_SOCKET_MODE: u32 = 0o600;

fn main() {
    SchemaBuild::from_environment().run();
}

struct SchemaBuild {
    crate_root: PathBuf,
}

impl SchemaBuild {
    fn from_environment() -> Self {
        Self {
            crate_root: PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir set")),
        }
    }

    fn run(&self) {
        println!("cargo:rerun-if-changed=schema/daemon.schema");
        println!("cargo:rerun-if-changed=src/schema/daemon.rs");

        let plan = GenerationPlan::new(&self.crate_root, "terminal", "0.1.0").with_module(
            ModuleEmission::daemon_module("daemon", Self::daemon_shape()),
        );
        GenerationDriver::new(plan)
            .generate()
            .expect("generate terminal schema artifacts")
            .write_or_check("TERMINAL_UPDATE_SCHEMA_ARTIFACTS")
            .expect("checked-in terminal schema artifacts are fresh");
    }

    /// The ordinary communication socket carries `signal-terminal` Signal
    /// frames the component decodes itself; the owner-only meta socket
    /// carries `meta-signal-terminal`. Both are bound by the emitted
    /// async listener runtime.
    fn daemon_shape() -> NexusDaemonShape {
        NexusDaemonShape::new(
            "terminal-supervisor",
            WorkingListenerTier::component_decoded(),
        )
        .with_meta_tier(MetaListenerTier::new(SocketModeBits::new(META_SOCKET_MODE)))
    }
}
