fn main() {
    println!("cargo:rerun-if-changed=src/llama_shim.c");
    let llama = pkg_config::Config::new()
        .atleast_version("0.0.10273")
        .probe("llama")
        .expect("the pinned nixpkgs llama-cpp package must be available through pkg-config");
    let mut build = cc::Build::new();
    build.file("src/llama_shim.c").warnings(true).opt_level(1);
    for include in llama.include_paths {
        build.include(include);
    }
    build.compile("beankey_llama_shim");
}
