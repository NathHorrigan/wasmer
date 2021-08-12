//! Defining an engine in Wasmer is one of the fundamental steps.
//!
//! This example builds on that of 'engine_headless.rs' but instead of
//! serializing a module and then deserializing it again for your host machines target,
//! We instead create an engine for our target architecture (In this case an ARM64 iOS device),
//! serialize a simple module to a .dylib file that can be copied to an iOS project and
//! deserialized/ran using the 'Headless C-API'.
//!
//! ```shell
//! cargo run --example platform-headless-ios --release --features "cranelift"
//! ```
//!
//! Ready?

use std::fs::File;
use std::path::Path;
use std::str::FromStr;
use tempfile::NamedTempFile;
use wasmer::imports;
use wasmer::wat2wasm;
use wasmer::Instance;
use wasmer::Module;
use wasmer::RuntimeError;
use wasmer::Store;
use wasmer::Value;
use wasmer_compiler::{CpuFeature, Target, Triple};
use wasmer_compiler_cranelift::Cranelift;
use wasmer_engine_dylib::Dylib;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Let's declare the Wasm module with the text representation.
    let wasm_bytes = wat2wasm(
        r#"
(module
(type $sum_t (func (param i32 i32) (result i32)))
(func $sum_f (type $sum_t) (param $x i32) (param $y i32) (result i32)
local.get $x
local.get $y
i32.add)
(export "sum" (func $sum_f)))
"#
        .as_bytes(),
    )?;

    // Create a compiler for iOS
    let compiler_config = Cranelift::default();
    let triple = Triple::from_str("aarch64-apple-ios")
        .map_err(|error| RuntimeError::new(error.to_string()))?;

    // Let's build the target.
    let mut cpu_feature = CpuFeature::set();
    let target = Target::new(triple, cpu_feature);
    println!("Chosen target: {:?}", target);

    println!("Creating Dylib engine...");
    let engine = Dylib::new(compiler_config).target(target).engine();

    // Create a store, that holds the engine.
    let store = Store::new(&engine);

    println!("Compiling module...");
    // Let's compile the Wasm module.
    let module = Module::new(&store, wasm_bytes)?;
    // Here we go. Let's serialize the compiled Wasm module in a
    // file.
    println!("Serializing module...");
    let mut dylib_file = Path::new("./sum.dylib");
    module.serialize_to_file(dylib_file)?;

    Ok(())
}

#[test]
#[cfg(target_os = "macos")]
fn test_engine_headless_ios() -> Result<(), Box<dyn std::error::Error>> {
    main()
}
