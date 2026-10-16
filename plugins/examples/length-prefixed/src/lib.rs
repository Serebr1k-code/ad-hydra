//! u16 big-endian length prefix; state retains a bounded incomplete frame.
use hydra_protocol_sdk::*;
use std::collections::BTreeMap;
#[unsafe(no_mangle)]
pub extern "C" fn hydra_abi_version() -> i32 {
    ABI_VERSION
}
#[unsafe(no_mangle)]
pub extern "C" fn hydra_alloc(len: i32) -> i32 {
    let data = vec![0u8; len as usize].into_boxed_slice();
    Box::into_raw(data) as *mut u8 as i32
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hydra_decode(ptr: i32, len: i32) -> i64 {
    let input: DecodeInput = serde_json::from_slice(unsafe {
        std::slice::from_raw_parts(ptr as *const u8, len as usize)
    })
    .unwrap();
    let mut buffer = input.state;
    buffer.extend_from_slice(&input.bytes);
    assert!(buffer.len() <= MAX_STATE);
    let mut messages = vec![];
    while buffer.len() >= 2 {
        let len = u16::from_be_bytes([buffer[0], buffer[1]]) as usize;
        assert!(len <= 8192);
        if buffer.len() < len + 2 {
            break;
        }
        messages.push(ParsedMessage {
            protocol: "length-prefixed".into(),
            message_type: "frame".into(),
            direction: input.direction,
            fields: BTreeMap::from([("protocol.payload".into(), Value::Bytes(buffer[2..len + 2].to_vec()))]),
        });
        buffer.drain(..len + 2);
    }
    let output = serde_json::to_vec(&DecodeOutput {
        messages,
        state: buffer,
    })
    .unwrap()
    .into_boxed_slice();
    let size = output.len();
    let address = Box::into_raw(output) as *mut u8 as u32;
    (((address as u64) << 32) | size as u64) as i64
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hydra_dealloc(input: i32, input_len: i32, output: i32, output_len: i32) {
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            input as *mut u8,
            input_len as usize,
        )));
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            output as *mut u8,
            output_len as usize,
        )));
    }
}

/// Optional ABI v1 metadata extension; static module-owned JSON.
#[unsafe(no_mangle)]
pub extern "C" fn hydra_metadata() -> i64 {
    static METADATA: &[u8] = br#"{"name":"length-prefixed","fields":[{"path":"protocol.payload","field_type":"bytes"}]}"#;
    (((METADATA.as_ptr() as u32 as u64) << 32) | METADATA.len() as u64) as i64
}
