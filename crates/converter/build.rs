fn main() {
    println!("cargo:rerun-if-changed=src/ngram_marisa.cc");
    let marisa = pkg_config::Config::new()
        .atleast_version("0.3")
        .cargo_metadata(false)
        .probe("marisa")
        .expect("marisa-trie is required to build beankey-converter");
    let mut compiler = cc::Build::new();
    compiler
        .cpp(true)
        .std("c++17")
        .opt_level(1)
        .file("src/ngram_marisa.cc")
        .warnings(true);
    for include in marisa.include_paths {
        compiler.include(include);
    }
    compiler.compile("beankey-ngram-marisa");
    for link_path in marisa.link_paths {
        println!("cargo:rustc-link-search=native={}", link_path.display());
    }
    println!("cargo:rustc-link-lib=marisa");
}
