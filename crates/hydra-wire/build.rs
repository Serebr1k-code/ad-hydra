fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto = "../../contracts/hydra/proxy/v1/proxy.proto";
    println!("cargo:rerun-if-changed={proto}");
    tonic_build::configure().compile_protos(&[proto], &["../../contracts"])?;
    Ok(())
}
