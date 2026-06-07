use std::{env, path::PathBuf};

use schema_rust_next::build::{GenerationDriver, GenerationPlan, ModuleEmission};

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
        println!("cargo:rerun-if-changed=schema/signal.schema");
        println!("cargo:rerun-if-changed=src/schema/signal.rs");
        println!("cargo:rerun-if-changed=schema/sema.schema");
        println!("cargo:rerun-if-changed=src/schema/sema.rs");
        println!("cargo:rerun-if-changed=schema/nexus.schema");
        println!("cargo:rerun-if-changed=src/schema/nexus.rs");

        let plan = GenerationPlan::new(&self.crate_root, "terminal", "0.1.0")
            .with_module(ModuleEmission::signal_runtime_module("signal"))
            .with_module(ModuleEmission::sema_runtime())
            .with_module(ModuleEmission::nexus_runtime());
        GenerationDriver::new(plan)
            .generate()
            .expect("generate terminal schema artifacts")
            .write_or_check("TERMINAL_UPDATE_SCHEMA_ARTIFACTS")
            .expect("checked-in terminal schema artifacts are fresh");
    }
}
