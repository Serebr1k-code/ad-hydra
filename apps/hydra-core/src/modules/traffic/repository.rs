use crate::modules::auth::AuthState;
use hydra_wire::v1::{self as proto, event::Payload};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::{Semaphore, broadcast};
#[derive(Clone)]
pub struct Traffic {
    pub(crate) auth: AuthState,
    pub(crate) events: broadcast::Sender<Value>,
    pub(crate) streams: Arc<Semaphore>,
}
#[derive(Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in=Query)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub after: Option<i64>,
    pub limit: Option<usize>,
    pub service_id: Option<String>,
    pub client_ip: Option<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    /// 1 = TCP, 2 = UDP.
    pub transport: Option<i32>,
    /// Opening flow client endpoint; independent of message direction.
    pub source_ip: Option<String>,
    /// Opening flow upstream endpoint.
    pub destination_ip: Option<String>,
    /// 1 = client to server, 2 = server to client.
    pub direction: Option<i32>,
    /// Exact parser field identifier (for example http.path).
    pub field_path: Option<String>,
    /// Exact scalar text value; requires field_path.
    pub field_value: Option<String>,
    pub matched_rule: Option<String>,
    /// Matches durable critical finding provenance without returning flag values.
    pub has_flag: Option<bool>,
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct Page {
    #[schema(value_type=Vec<Object>)]
    pub items: Vec<Value>,
    pub next_cursor: Option<i64>,
}
#[derive(Default)]
pub struct IngestResult {
    pub accepted: usize,
    pub duplicates: usize,
}
impl Traffic {
    pub async fn open(auth: AuthState) -> anyhow::Result<Self> {
        auth.db
            .migrate(
                3,
                include_str!("../../../../../migrations/0003_traffic.sql"),
            )
            .await?;
        let (events, _) = broadcast::channel(128);
        Ok(Self {
            auth,
            events,
            streams: Arc::new(Semaphore::new(8)),
        })
    }
    pub async fn ingest(&self, batch: proto::EventBatch) -> anyhow::Result<IngestResult> {
        anyhow::ensure!(batch.events.len() <= 256, "batch exceeds 256 events");
        // Convert and bound the complete batch before entering its atomic write transaction.
        let rows = batch
            .events
            .into_iter()
            .map(convert)
            .collect::<anyhow::Result<Vec<_>>>()?;
        let (result,published)=self.auth.db.run(move|db|{let tx=db.transaction()?;
   let incoming=rows.iter().map(|r|r.data.len() as i64*3).sum::<i64>();
   for _ in 0..256 {
    let used:i64=tx.query_row("SELECT bytes FROM traffic_budget WHERE id=1",[],|r|r.get(0))?;
    if used+incoming<=64*1024*1024 {break;}
    for table in ["traffic_events","traffic_messages","traffic_flows"] {tx.execute(&format!("DELETE FROM {table} WHERE seq IN (SELECT seq FROM {table} ORDER BY seq LIMIT 1000)"),[])?;}
   }
   let used:i64=tx.query_row("SELECT bytes FROM traffic_budget WHERE id=1",[],|r|r.get(0))?;
   anyhow::ensure!(used+incoming<=64*1024*1024,"traffic storage quota exhausted");
   let mut result=IngestResult::default();let mut published=Vec::new();
   for mut row in rows {
    let at:Option<String>=tx.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ',?1/1000.0,'unixepoch')",[row.at],|r|r.get(0))?;
    let mut data:Value=serde_json::from_str(&row.data)?;data["occurred_at"]=at.ok_or_else(||anyhow::anyhow!("event time outside supported range"))?.into();row.data=serde_json::to_string(&data)?;
    let n=tx.execute("INSERT OR IGNORE INTO traffic_events(event_id,flow_id,kind,data) VALUES(?1,?2,?3,?4)",params![row.event_id,row.flow_id,row.kind,row.data])?;
    if n==0 {result.duplicates+=1;continue;}result.accepted+=1;
    match row.kind {
     "flow_opened"=>{tx.execute("INSERT OR IGNORE INTO traffic_flows(id,service_id,occurred_ms,client_ip,data) VALUES(?1,?2,?3,?4,?5)",params![row.flow_id,row.service_id,row.at,row.client,row.data])?;},
     "flow_closed"=>{tx.execute("UPDATE traffic_flows SET data=json_set(data,'$.closed',json(?2)) WHERE id=?1",params![row.flow_id,row.data])?;},
     "message"=>{tx.execute("INSERT OR IGNORE INTO traffic_messages(id,flow_id,service_id,occurred_ms,data) VALUES(?1,?2,?3,?4,?5)",params![row.message_id,row.flow_id,row.service_id,row.at,row.data])?;},_=>{}
    }
    // SSE contains parsed fields, never raw sample content.
    published.push(serde_json::from_str::<Value>(&row.data)?);
   }
   tx.execute("DELETE FROM traffic_messages WHERE seq<=(SELECT COALESCE(MAX(seq),0)-100000 FROM traffic_messages)",[])?;
   tx.execute("DELETE FROM traffic_flows WHERE seq<=(SELECT COALESCE(MAX(seq),0)-10000 FROM traffic_flows)",[])?;
   tx.execute("DELETE FROM traffic_events WHERE seq<=(SELECT COALESCE(MAX(seq),0)-100000 FROM traffic_events)",[])?;
   tx.commit()?;Ok((result,published))}).await?;
        for event in published {
            let _ = self.events.send(event);
        }
        Ok(result)
    }
    pub async fn flows(&self, q: Query) -> anyhow::Result<Page> {
        self.list(q, None).await
    }
    pub async fn messages(&self, flow: String, q: Query) -> anyhow::Result<Page> {
        anyhow::ensure!(flow.len() <= 128, "invalid flow id");
        self.list(q, Some(flow)).await
    }
    async fn list(&self, q: Query, flow: Option<String>) -> anyhow::Result<Page> {
        let limit = q.limit.unwrap_or(50);
        anyhow::ensure!(
            (1..=100).contains(&limit) && q.after.unwrap_or(0) >= 0,
            "invalid pagination"
        );
        anyhow::ensure!(
            q.service_id.as_ref().is_none_or(|s| s.len() <= 128)
                && q.client_ip.as_ref().is_none_or(|s| s.len() <= 64),
            "invalid filter"
        );
        anyhow::ensure!(
            q.since_ms.zip(q.until_ms).is_none_or(|(a, b)| a <= b),
            "invalid time range"
        );
        anyhow::ensure!(
            q.transport.is_none_or(|v| (1..=2).contains(&v))
                && q.direction.is_none_or(|v| (1..=2).contains(&v)),
            "invalid transport or direction"
        );
        for value in [
            &q.source_ip,
            &q.destination_ip,
            &q.field_path,
            &q.matched_rule,
        ] {
            anyhow::ensure!(
                value
                    .as_ref()
                    .is_none_or(|v| !v.is_empty() && v.len() <= 128),
                "invalid filter"
            );
        }
        anyhow::ensure!(
            q.field_path
                .as_ref()
                .is_none_or(|v| v.split('.').count() <= 16)
                && q.field_value.as_ref().is_none_or(|v| v.len() <= 1024)
                && (q.field_value.is_none() || q.field_path.is_some()),
            "invalid field query"
        );
        anyhow::ensure!(
            q.client_ip
                .as_ref()
                .zip(q.source_ip.as_ref())
                .is_none_or(|(a, b)| a == b),
            "conflicting source filters"
        );
        self.auth.db.run(move |db| {
            use rusqlite::types::Value as Sql;
            let messages=flow.is_some();
            let table=if messages {"traffic_messages"} else {"traffic_flows"};
            let flow_ref=if messages {"t.flow_id"} else {"t.id"};
            let mut sql=format!("SELECT t.seq,t.data FROM {table} t WHERE t.seq>?");
            let mut values=vec![Sql::Integer(q.after.unwrap_or(0))];
            { let mut add=|clause:&str,value:Sql| {sql.push_str(clause);values.push(value);};
            if let Some(v)=flow {add(" AND t.flow_id=?",v.into());}
            if let Some(v)=q.service_id {add(" AND t.service_id=?",v.into());}
            if let Some(v)=q.since_ms {add(" AND t.occurred_ms>=?",v.into());}
            if let Some(v)=q.until_ms {add(" AND t.occurred_ms<=?",v.into());}
            // Endpoint predicates refer to the indexed opening flow, not a guessed message direction.
            if let Some(v)=q.client_ip.or(q.source_ip) {add(&if messages {format!(" AND EXISTS(SELECT 1 FROM traffic_flows f WHERE f.id={flow_ref} AND f.client_ip=?)")}else{" AND t.client_ip=?".into()},v.into());}
            if let Some(v)=q.destination_ip {add(&if messages {format!(" AND EXISTS(SELECT 1 FROM traffic_flows f WHERE f.id={flow_ref} AND json_extract(f.data,'$.upstream.ip')=?)")}else{" AND json_extract(t.data,'$.upstream.ip')=?".into()},v.into());}
            if let Some(v)=q.transport {add(&if messages {format!(" AND EXISTS(SELECT 1 FROM traffic_flows f WHERE f.id={flow_ref} AND json_extract(f.data,'$.transport')=?)")}else{" AND json_extract(t.data,'$.transport')=?".into()},Sql::Integer(v.into()));}
            if let Some(v)=q.matched_rule {add(&format!(" AND EXISTS(SELECT 1 FROM traffic_events e WHERE e.flow_id={flow_ref} AND e.kind='rule_hit' AND json_extract(e.data,'$.rule_id')=?)"),v.into());}
            }
            if let Some(v)=q.has_flag {
                sql.push_str(&format!(" AND {}EXISTS(SELECT 1 FROM flag_events fe WHERE fe.flow_id={flow_ref})",if v {""}else{"NOT "}));
            }
            if q.direction.is_some() || q.field_path.is_some() {
                let predicate=if messages {"m.id=t.id".to_owned()}else{"m.flow_id=t.id".to_owned()};
                sql.push_str(&format!(" AND EXISTS(SELECT 1 FROM traffic_messages m WHERE {predicate}"));
                if let Some(v)=q.direction {sql.push_str(" AND json_extract(m.data,'$.direction')=?");values.push(Sql::Integer(v.into()));}
                if let Some(v)=q.field_path {
                    sql.push_str(" AND EXISTS(SELECT 1 FROM json_each(m.data,'$.fields') field WHERE field.key=?");values.push(v.into());
                    if let Some(v)=q.field_value {sql.push_str(" AND CAST(json_extract(field.value,'$.value') AS TEXT)=?");values.push(v.into());}
                    sql.push(')');
                }
                sql.push(')');
            }
            sql.push_str(" ORDER BY t.seq LIMIT ?");values.push(Sql::Integer((limit+1) as i64));
            let mut stmt=db.prepare(&sql)?;
            let rows=stmt.query_map(rusqlite::params_from_iter(values),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
            let next_cursor=if rows.len()>limit {Some(rows[limit-1].0)}else{None};
            let items=rows.into_iter().take(limit).map(|(_,data)|serde_json::from_str(&data)).collect::<Result<Vec<_>,_>>()?;
            Ok(Page{items,next_cursor})
        }).await
    }

    pub async fn flow(&self, id: String) -> anyhow::Result<Option<Value>> {
        let Some(mut value) = self.one("traffic_flows", id.clone()).await? else {
            return Ok(None);
        };
        let (findings,has_flag)=self.auth.db.run(move |db| {
            let mut statement=db.prepare("SELECT data FROM traffic_events WHERE flow_id=?1 AND kind='rule_hit' ORDER BY seq LIMIT 101")?;
            let findings=statement.query_map([&id],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            let has_flag:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM flag_events WHERE flow_id=?1)",[id],|r|r.get(0))?;
            Ok((findings,has_flag))
        }).await?;
        value["findings_truncated"] = (findings.len() > 100).into();
        value["rule_hits"] = Value::Array(
            findings
                .into_iter()
                .take(100)
                .map(|v| serde_json::from_str(&v))
                .collect::<Result<Vec<_>, _>>()?,
        );
        value["has_flag"] = has_flag.into();
        Ok(Some(value))
    }
    pub async fn message(&self, id: String) -> anyhow::Result<Option<Value>> {
        self.one("traffic_messages", id).await
    }
    async fn one(&self, table: &'static str, id: String) -> anyhow::Result<Option<Value>> {
        anyhow::ensure!(id.len() <= 128, "invalid id");
        self.auth
            .db
            .run(move |db| {
                let value = db
                    .query_row(
                        &format!("SELECT data FROM {table} WHERE id=?1"),
                        [id],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?;
                Ok(value.map(|s| serde_json::from_str(&s)).transpose()?)
            })
            .await
    }
}
struct Row {
    event_id: String,
    flow_id: String,
    service_id: String,
    at: i64,
    kind: &'static str,
    client: String,
    message_id: String,
    data: String,
}
fn convert(event: proto::Event) -> anyhow::Result<Row> {
    anyhow::ensure!(
        !event.event_id.is_empty()
            && event.event_id.len() <= 128
            && event.flow_id.len() <= 128
            && event.service_id.len() <= 128,
        "invalid event metadata"
    );
    let at = event
        .occurred_at
        .map(|t| {
            t.seconds
                .saturating_mul(1000)
                .saturating_add(i64::from(t.nanos) / 1_000_000)
        })
        .unwrap_or(0);
    let mut client = String::new();
    let mut message_id = String::new();
    let (kind, payload) = match event.payload {
        Some(Payload::FlowOpened(f)) => {
            client = f.client.as_ref().map(|e| e.ip.clone()).unwrap_or_default();
            (
                "flow_opened",
                json!({"transport":f.transport,"client":f.client.map(|e|json!({"ip":e.ip,"port":e.port})),"upstream":f.upstream.map(|e|json!({"ip":e.ip,"port":e.port}))}),
            )
        }
        Some(Payload::FlowClosed(f)) => (
            "flow_closed",
            json!({"bytes_client_to_server":f.bytes_client_to_server.to_string(),"bytes_server_to_client":f.bytes_server_to_client.to_string(),"reason":f.reason}),
        ),
        Some(Payload::Message(m)) => {
            anyhow::ensure!(
                !m.message_id.is_empty() && m.message_id.len() <= 128,
                "invalid message id"
            );
            message_id = m.message_id.clone();
            let mut budget = 0;
            let fields = m
                .fields
                .map(|f| {
                    f.fields
                        .into_iter()
                        .map(|(k, v)| {
                            budget += k.len();
                            Ok((k, field(v, 1, &mut budget)?))
                        })
                        .collect::<anyhow::Result<serde_json::Map<String, Value>>>()
                })
                .transpose()?
                .unwrap_or_default();
            (
                "message",
                json!({"message_id":m.message_id,"protocol_id":m.protocol_id,"message_type":m.message_type,"direction":m.direction,"fields":fields,"sample_id":m.sample_id,"original_size":m.original_size,"truncated":m.truncated}),
            )
        }
        Some(Payload::RuleHit(h)) => (
            "rule_hit",
            json!({"rule_id":h.rule_id,"action":h.action,"mode":h.mode,"message_id":h.message_id}),
        ),
        Some(Payload::Loss(l)) => (
            "event_loss",
            json!({"count":l.count.to_string(),"reason":l.reason}),
        ),
        _ => anyhow::bail!("unsupported traffic event"),
    };
    let mut data = payload;
    let object = data.as_object_mut().unwrap();
    object.insert("event_id".into(), event.event_id.clone().into());
    object.insert("flow_id".into(), event.flow_id.clone().into());
    object.insert("service_id".into(), event.service_id.clone().into());
    object.insert("occurred_ms".into(), at.into());
    object.insert("kind".into(), kind.into());
    let data = serde_json::to_string(&data)?;
    anyhow::ensure!(data.len() <= 65536, "event row exceeds 64 KiB");
    Ok(Row {
        event_id: event.event_id,
        flow_id: event.flow_id,
        service_id: event.service_id,
        at,
        kind,
        client,
        message_id,
        data,
    })
}
pub(super) fn field(
    value: proto::FieldValue,
    depth: usize,
    budget: &mut usize,
) -> anyhow::Result<Value> {
    anyhow::ensure!(depth <= 16 && *budget <= 32768, "field complexity exceeded");
    *budget += 1;
    use proto::field_value::Value as F;
    let (kind, value) = match value.value {
        Some(F::StringValue(s)) => {
            *budget += s.len();
            ("string", json!(s))
        }
        Some(F::IntValue(n)) => ("int", json!(n.to_string())),
        Some(F::UintValue(n)) => ("uint", json!(n.to_string())),
        Some(F::DoubleValue(n)) => {
            anyhow::ensure!(n.is_finite(), "invalid double");
            ("double", json!(n))
        }
        Some(F::BoolValue(b)) => ("bool", json!(b)),
        Some(F::BytesValue(b)) => {
            *budget += b.len();
            ("bytes", json!(b))
        }
        Some(F::ObjectValue(o)) => {
            let mut fields = serde_json::Map::new();
            for (k, v) in o.fields {
                *budget += k.len();
                fields.insert(k, field(v, depth + 1, budget)?);
            }
            ("object", Value::Object(fields))
        }
        Some(F::ListValue(l)) => (
            "list",
            Value::Array(
                l.values
                    .into_iter()
                    .map(|v| field(v, depth + 1, budget))
                    .collect::<anyhow::Result<_>>()?,
            ),
        ),
        None => anyhow::bail!("missing typed field"),
    };
    anyhow::ensure!(*budget <= 32768, "field data exceeds limit");
    Ok(json!({"type":kind,"value":value}))
}

/// Return typed fields only from a durably stored parsed message, for filter rehearsal.
pub async fn saved_fields(
    db: &crate::infrastructure::db::Database,
    id: String,
) -> anyhow::Result<std::collections::BTreeMap<String, hydra_domain::FieldValue>> {
    anyhow::ensure!(!id.is_empty() && id.len() <= 128, "invalid message id");
    db.run(move |db| {
        let text: String =
            db.query_row("SELECT data FROM traffic_messages WHERE id=?1", [id], |r| {
                r.get(0)
            })?;
        let data: Value = serde_json::from_str(&text)?;
        let fields = data["fields"]
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("saved message has no fields"))?;
        fn convert(v: &Value, depth: usize) -> anyhow::Result<hydra_domain::FieldValue> {
            anyhow::ensure!(depth <= 16, "field depth");
            use hydra_domain::FieldValue as F;
            let value = &v["value"];
            Ok(match v["type"].as_str().unwrap_or_default() {
                "string" => F::String(
                    value
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("string field"))?
                        .into(),
                ),
                "int" => F::Int(
                    value
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("int field"))?
                        .parse()?,
                ),
                "uint" => F::Uint(
                    value
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("uint field"))?
                        .parse()?,
                ),
                "double" => F::Double(
                    value
                        .as_f64()
                        .ok_or_else(|| anyhow::anyhow!("double field"))?,
                ),
                "bool" => F::Bool(
                    value
                        .as_bool()
                        .ok_or_else(|| anyhow::anyhow!("bool field"))?,
                ),
                "bytes" => F::Bytes(serde_json::from_value(value.clone())?),
                "object" => F::Object(
                    value
                        .as_object()
                        .ok_or_else(|| anyhow::anyhow!("object field"))?
                        .iter()
                        .map(|(k, v)| Ok((k.clone(), convert(v, depth + 1)?)))
                        .collect::<anyhow::Result<_>>()?,
                ),
                "list" => F::List(
                    value
                        .as_array()
                        .ok_or_else(|| anyhow::anyhow!("list field"))?
                        .iter()
                        .map(|v| convert(v, depth + 1))
                        .collect::<anyhow::Result<_>>()?,
                ),
                _ => anyhow::bail!("unsupported stored field"),
            })
        }
        fields
            .iter()
            .map(|(k, v)| Ok((k.clone(), convert(v, 0)?)))
            .collect()
    })
    .await
}

pub async fn saved_context(
    db: &crate::infrastructure::db::Database,
    id: String,
) -> anyhow::Result<(
    String,
    std::collections::BTreeMap<String, hydra_domain::FieldValue>,
)> {
    let fields = saved_fields(db, id.clone()).await?;
    let service = db
        .run(move |db| {
            Ok(db.query_row(
                "SELECT service_id FROM traffic_messages WHERE id=?1",
                [id],
                |r| r.get::<_, String>(0),
            )?)
        })
        .await?;
    Ok((service, fields))
}
