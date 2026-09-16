fn main() {
    println!("cargo:rerun-if-changed=../../proto/beankey.proto");
    prost_build::compile_protos(&["../../proto/beankey.proto"], &["../../proto"])
        .expect("beanKey protocol generation failed");
}
