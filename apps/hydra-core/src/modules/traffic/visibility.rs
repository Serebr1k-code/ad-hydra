//! Retained classification uses actual enforcement, never response status or shadow matches.
pub(super) fn blocked_sql(flow: &str, message: Option<&str>) -> String {
    let mut sql = format!(
        "EXISTS(SELECT 1 FROM traffic_events e WHERE e.flow_id={flow} AND e.kind='rule_hit' AND (json_extract(e.data,'$.blocked')=1 OR (json_extract(e.data,'$.mode')=2 AND json_extract(e.data,'$.action') IN (3,4,5)))"
    );
    if let Some(message) = message {
        sql.push_str(&format!(" AND (COALESCE(json_extract(e.data,'$.message_id'),'')='' OR json_extract(e.data,'$.message_id')={message})"));
    }
    sql.push(')');
    sql
}
