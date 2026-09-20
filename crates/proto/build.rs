use std::io::Result;

fn main() -> Result<()> {
    // Use the vendored protoc so contributors don't need a system install.
    if std::env::var_os("PROTOC").is_none() {
        if let Ok(path) = protoc_bin_vendored::protoc_bin_path() {
            std::env::set_var("PROTOC", path);
        }
    }

    let proto_files = &[
        "../../proto/common.proto",
        "../../proto/telemetry.proto",
        "../../proto/control.proto",
    ];

    for proto in proto_files {
        println!("cargo:rerun-if-changed={proto}");
    }
    println!("cargo:rerun-if-changed=../../proto");

    tonic_build::configure()
        .build_server(false)
        .build_client(false)
        .compile_protos(proto_files, &["../../proto"])?;

    Ok(())
}
