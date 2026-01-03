//! Build script for generating Rust code from Protocol Buffers

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Compile proto files
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile(
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
