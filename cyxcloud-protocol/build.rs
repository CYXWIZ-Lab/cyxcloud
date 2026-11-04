//! Build script for generating Rust code from Protocol Buffers

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Compile proto files
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(
            &[
                "proto/chunk.proto",
                "proto/metadata.proto",
                "proto/node.proto",
                "proto/data.proto",
                "proto/datastream.proto",
            ],
            &["proto"],
        )?;

    Ok(())
}
