//! WASI request checker. Build with rustc --edition 2024 --target wasm32-wasip1
//! -O -C strip=symbols main.rs -o checker.wasm.
use std::{
    fs,
    io::{self, Read, Write},
};

const LIMIT_FILE: &str = "/state/byte-limit";
const MAX_REQUEST: usize = 1024 * 1024;

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut header = [0; 4];
    let mut request = Vec::new();
    while input.read(&mut header[..1])? != 0 {
        input.read_exact(&mut header[1..])?;
        let size = u32::from_be_bytes(header) as usize;
        if size > MAX_REQUEST {
            return Err(io::Error::other("request length exceeds limit"));
        }
        request.resize(size, 0);
        input.read_exact(&mut request)?;
        let limit = match fs::read_to_string(LIMIT_FILE) {
            Ok(value) => value,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                fs::write(LIMIT_FILE, "65536\n")?;
                "65536".into()
            }
            Err(err) => return Err(err),
        };
        let limit: usize = limit.trim().parse().map_err(io::Error::other)?;
        output.write_all(&[u8::from(request.len() <= limit)])?;
        output.flush()?;
    }
    Ok(())
}
