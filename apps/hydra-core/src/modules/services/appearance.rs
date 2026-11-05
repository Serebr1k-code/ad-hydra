//! Core-only presentation settings; changing colors never restarts a relay.
use super::Control;
use crate::modules::{audit, audit::Actor, auth::now};
use rusqlite::OptionalExtension;
use std::collections::BTreeMap;

impl Control {
    pub async fn service_colors(&self) -> anyhow::Result<BTreeMap<String, String>> {
        self.db.run(|db| {
            Ok(db.prepare("SELECT service_id,color FROM service_appearance ORDER BY service_id LIMIT 128")?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?)
        }).await
    }

    pub async fn set_service_color(
        &self,
        actor: Actor,
        id: String,
        color: Option<String>,
        request: String,
    ) -> anyhow::Result<()> {
        let color = color.map(|value| value.to_ascii_lowercase());
        anyhow::ensure!(
            color.as_ref().is_none_or(|v| v.len() == 7
                && v.starts_with('#')
                && v.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)),
            "color must be #RRGGBB or null to reset"
        );
        self.db.run(move |db| {
            let tx = db.transaction()?;
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM control_attribution WHERE kind='service' AND id=?1)", [&id], |r| r.get(0))?;
            anyhow::ensure!(exists, "service not found");
            let before: Option<String> = tx.query_row("SELECT color FROM service_appearance WHERE service_id=?1", [&id], |r| r.get(0)).optional()?;
            if let Some(value) = &color {
                tx.execute("INSERT INTO service_appearance(service_id,color) VALUES(?1,?2) ON CONFLICT(service_id) DO UPDATE SET color=excluded.color", rusqlite::params![id,value])?;
            } else {
                tx.execute("DELETE FROM service_appearance WHERE service_id=?1", [&id])?;
            }
            audit::append_change(&tx, &actor, "service.color", &id, &request, true, now(), &serde_json::json!({"before":{"color":before},"after":{"color":color}}))?;
            tx.commit()?;
            Ok(())
        }).await
    }
}

pub(super) fn normalize_name(value: String) -> anyhow::Result<String> {
    let value = value.trim().to_owned();
    anyhow::ensure!(
        !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control),
        "service name must be 1..256 UTF-8 bytes without control characters"
    );
    Ok(value)
}
pub(super) fn write_name(
    tx: &rusqlite::Transaction<'_>,
    actor: &Actor,
    id: &str,
    name: &str,
    request: &str,
) -> anyhow::Result<()> {
    let before: Option<String> = tx
        .query_row(
            "SELECT name FROM service_names WHERE service_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    if before.as_deref() == Some(name) {
        return Ok(());
    }
    tx.execute("INSERT INTO service_names(service_id,name) VALUES(?1,?2) ON CONFLICT(service_id) DO UPDATE SET name=excluded.name",rusqlite::params![id,name])?;
    audit::append_change(
        tx,
        actor,
        "service.name",
        id,
        request,
        true,
        now(),
        &serde_json::json!({"before":{"name":before},"after":{"name":name}}),
    )?;
    Ok(())
}
impl Control {
    pub async fn service_names(&self) -> anyhow::Result<BTreeMap<String, String>> {
        self.db
            .run(|db| {
                Ok(db
                    .prepare(
                        "SELECT service_id,name FROM service_names ORDER BY service_id LIMIT 128",
                    )?
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<_, _>>()?)
            })
            .await
    }
}
