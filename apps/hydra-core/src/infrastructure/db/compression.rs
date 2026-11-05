use rusqlite::{Connection, functions::FunctionFlags};

/// SQLite decoded-traffic views expand one bounded record, never an entire archive.
pub(super) fn register(connection: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    connection.create_scalar_function("hydra_zstd", 1, flags, |ctx| {
        let text = ctx.get::<String>(0)?;
        if text.len() > 65536 {
            return Err(rusqlite::Error::UserFunctionError(
                "decoded record exceeds 64 KiB".into(),
            ));
        }
        hydra_compression::encode_at_level(text.as_bytes(), 3)
            .map_err(|e| rusqlite::Error::UserFunctionError(e.into()))
    })?;
    connection.create_scalar_function("hydra_text", 1, flags, |ctx| {
        let data = ctx.get::<Vec<u8>>(0)?;
        if data.len() > 65792 {
            return Err(rusqlite::Error::UserFunctionError(
                "compressed record exceeds limit".into(),
            ));
        }
        let decoded = hydra_compression::decode(&data, 65536)
            .map_err(|e| rusqlite::Error::UserFunctionError(e.into()))?;
        String::from_utf8(decoded).map_err(|e| rusqlite::Error::UserFunctionError(e.into()))
    })
}
