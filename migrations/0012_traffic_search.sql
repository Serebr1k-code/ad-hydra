-- Stable sequence pagination with selective endpoint, time and finding lookups.
CREATE INDEX traffic_flow_time ON traffic_flows(occurred_ms,seq);
CREATE INDEX traffic_flow_client ON traffic_flows(client_ip,seq);
CREATE INDEX traffic_flow_upstream ON traffic_flows(json_extract(data,'$.upstream.ip'),seq);
CREATE INDEX traffic_flow_transport ON traffic_flows(json_extract(data,'$.transport'),seq);
CREATE INDEX traffic_message_service ON traffic_messages(service_id,seq);
CREATE INDEX traffic_message_time ON traffic_messages(occurred_ms,seq);
CREATE INDEX traffic_message_direction ON traffic_messages(flow_id,json_extract(data,'$.direction'),seq);
CREATE INDEX traffic_rule_lookup ON traffic_events(flow_id,kind,json_extract(data,'$.rule_id'));
