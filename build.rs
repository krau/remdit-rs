use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if let Ok(default_server) = env::var("REMDIT_DEFAULT_SERVER") {
        println!("cargo:rustc-env=REMDIT_DEFAULT_SERVER={}", default_server);
    }

    println!("cargo:rerun-if-env-changed=REMDIT_DEFAULT_SERVER");
}
