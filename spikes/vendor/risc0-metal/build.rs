use std::{env, path::PathBuf, process::Command};

fn run(command: &mut Command) {
    assert!(command.status().expect("run Metal build tool").success(), "Metal build failed: {command:?}");
}

fn main() {
    println!("cargo:rerun-if-changed=generate.py");
    println!("cargo:rerun-if-changed=kernels/field.h");
    // Operations per kernel; see generate.py.
    println!("cargo:rerun-if-env-changed=R0_METAL_BUDGET");
    println!("cargo:rerun-if-env-changed=R0_METAL_STORE");
    println!("cargo:rerun-if-env-changed=R0_METAL_EXP");
    let budget = env::var("R0_METAL_BUDGET").unwrap_or_else(|_| "400".into());
    let store = env::var("R0_METAL_STORE").unwrap_or_else(|_| "16".into());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for (circuit, subdir) in [("rv32im", "zirgen/"), ("recursion", ""), ("keccak", "zirgen/")] {
        for file in ["poly_ext.rs", "taps.rs"] {
            println!("cargo:rerun-if-changed=../risc0-circuit-{circuit}/src/{subdir}{file}");
        }
    }
    println!("cargo:rerun-if-changed=kernels/neon.h");
    println!("cargo:rerun-if-env-changed=R0_NEON_BUDGET");
    run(Command::new("python3").arg("generate.py").arg(&out).arg(&budget).arg(&store));
    // The NEON evaluators (CPU share of each constraint evaluation), compiled side by side: the
    // generated files are large (keccak's takes ~2 minutes).
    std::thread::scope(|scope| {
        for name in ["rv32im", "recursion", "keccak"] {
            let out = &out;
            scope.spawn(move || {
                cc::Build::new()
                    .file(out.join(format!("{name}_neon.c")))
                    .include("kernels")
                    .opt_level(2)
                    .flag("-mcpu=apple-m1")
                    .warnings(false)
                    .compile(&format!("{name}_neon"));
            });
        }
    });
    // rv32im's accumulate step, ported from risc0's CUDA by tools/port_accum.py.
    println!("cargo:rerun-if-changed=kernels/rv32im_accum.metal");
    println!("cargo:rerun-if-changed=kernels/accum.h");
    let air = out.join("rv32im_accum.air");
    run(Command::new("xcrun").args(["-sdk", "macosx", "metal", "-O2", "-I", "kernels", "-I", "../risc0-sys/kernels/zkp/metal",
        "-w", "-c", "kernels/rv32im_accum.metal", "-o"]).arg(&air));
    run(Command::new("xcrun").args(["-sdk", "macosx", "metallib"]).arg(air).arg("-o").arg(out.join("rv32im_accum.metallib")));
    for name in ["rv32im", "recursion", "keccak"] {
        let air = out.join(format!("{name}.air"));
        run(Command::new("xcrun").args(["-sdk", "macosx", "metal", "-O3", "-I", "kernels", "-c"])
            .arg(out.join(format!("{name}.metal"))).arg("-o").arg(&air));
        run(Command::new("xcrun").args(["-sdk", "macosx", "metallib"]).arg(air)
            .arg("-o").arg(out.join(format!("{name}.metallib"))));
    }
}
