fn main() {
    println!("cargo:rerun-if-changed=../../ipc/beankey.proto");
    prost_build::compile_protos(&["../../ipc/beankey.proto"], &["../../ipc"])
        .expect("beanKey protocol generation failed");
}
