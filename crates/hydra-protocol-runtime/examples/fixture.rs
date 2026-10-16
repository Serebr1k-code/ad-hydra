//! Compile an explicitly labeled WAT test fixture, not a product parser.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let source = args.next().ok_or("source.wat required")?;
    let destination = args.next().ok_or("destination.wasm required")?;
    std::fs::write(destination, wat::parse_file(source)?)?;
    Ok(())
}
