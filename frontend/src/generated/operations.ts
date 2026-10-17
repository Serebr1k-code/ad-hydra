// Generated from contracts/openapi.yaml; do not edit.
export const operations = {
  "activate": {
    "method": "POST",
    "path": "/api/v1/protocols/{id}/{version}/activate"
  },
  "audit_events": {
    "method": "GET",
    "path": "/api/v1/audit"
  },
  "bulk_state": {
    "method": "PUT",
    "path": "/api/v1/filters/state"
  },
  "cancel": {
    "method": "POST",
    "path": "/api/v1/farm/runs/{id}/cancel"
  },
  "capture": {
    "method": "POST",
    "path": "/api/v1/traffic/flows/{id}/capture"
  },
  "create": {
    "method": "POST",
    "path": "/api/v1/flags"
  },
  "create_filter": {
    "method": "POST",
    "path": "/api/v1/filters"
  },
  "create_service": {
    "method": "POST",
    "path": "/api/v1/services"
  },
  "deactivate": {
    "method": "POST",
    "path": "/api/v1/users/{id}/deactivate"
  },
  "deactivate_protocol": {
    "method": "DELETE",
    "path": "/api/v1/protocols/activations/{service_id}"
  },
  "delete_exploit": {
    "method": "DELETE",
    "path": "/api/v1/farm/exploits/{id}"
  },
  "delete_filter": {
    "method": "DELETE",
    "path": "/api/v1/filters/{id}"
  },
  "delete_service": {
    "method": "DELETE",
    "path": "/api/v1/services/{id}"
  },
  "delete_target": {
    "method": "DELETE",
    "path": "/api/v1/farm/targets/{id}"
  },
  "details": {
    "method": "GET",
    "path": "/api/v1/farm/runs/{id}"
  },
  "diagnostic_probe": {
    "method": "POST",
    "path": "/api/v1/diagnostics/probe"
  },
  "exploit": {
    "method": "PUT",
    "path": "/api/v1/farm/exploits"
  },
  "exploits": {
    "method": "GET",
    "path": "/api/v1/farm/exploits"
  },
  "export_configuration": {
    "method": "GET",
    "path": "/api/v1/config/export"
  },
  "farm_service_delete": {
    "method": "DELETE",
    "path": "/api/v1/farm/services/{id}"
  },
  "farm_service_write": {
    "method": "PUT",
    "path": "/api/v1/farm/services"
  },
  "farm_services": {
    "method": "GET",
    "path": "/api/v1/farm/services"
  },
  "farm_team_delete": {
    "method": "DELETE",
    "path": "/api/v1/farm/teams/{id}"
  },
  "farm_team_write": {
    "method": "PUT",
    "path": "/api/v1/farm/teams"
  },
  "farm_teams": {
    "method": "GET",
    "path": "/api/v1/farm/teams"
  },
  "farm_upload": {
    "method": "PUT",
    "path": "/api/v1/farm/exploits/{id}/versions/{version}"
  },
  "filter_state": {
    "method": "PUT",
    "path": "/api/v1/filters/{id}/state"
  },
  "filters": {
    "method": "GET",
    "path": "/api/v1/filters"
  },
  "flags_import": {
    "method": "POST",
    "path": "/api/v1/flags/import"
  },
  "flags_list": {
    "method": "GET",
    "path": "/api/v1/flags"
  },
  "flags_page": {
    "method": "GET",
    "path": "/api/v1/flags/page"
  },
  "flow": {
    "method": "GET",
    "path": "/api/v1/traffic/flows/{id}"
  },
  "flow_history": {
    "method": "GET",
    "path": "/api/v1/traffic/flows/{id}/history"
  },
  "flows": {
    "method": "GET",
    "path": "/api/v1/traffic/flows"
  },
  "from_message": {
    "method": "POST",
    "path": "/api/v1/filters/from-message"
  },
  "get_filter": {
    "method": "GET",
    "path": "/api/v1/filters/{id}"
  },
  "list": {
    "method": "GET",
    "path": "/api/v1/protocols"
  },
  "list_users": {
    "method": "GET",
    "path": "/api/v1/users"
  },
  "login": {
    "method": "POST",
    "path": "/api/v1/auth/login"
  },
  "logout": {
    "method": "POST",
    "path": "/api/v1/auth/logout"
  },
  "logs": {
    "method": "GET",
    "path": "/api/v1/farm/runs/{id}/logs"
  },
  "me": {
    "method": "GET",
    "path": "/api/v1/auth/me"
  },
  "message": {
    "method": "GET",
    "path": "/api/v1/traffic/messages/{id}"
  },
  "messages": {
    "method": "GET",
    "path": "/api/v1/traffic/flows/{id}/messages"
  },
  "probe": {
    "method": "POST",
    "path": "/api/v1/services/{id}/probe"
  },
  "profile": {
    "method": "PUT",
    "path": "/api/v1/protocol-profiles/{id}/{version}"
  },
  "proxy": {
    "method": "GET",
    "path": "/api/v1/system/proxy"
  },
  "refresh": {
    "method": "POST",
    "path": "/api/v1/auth/refresh"
  },
  "retry": {
    "method": "POST",
    "path": "/api/v1/farm/runs/{id}/retry"
  },
  "rotate": {
    "method": "DELETE",
    "path": "/api/v1/farm/runs"
  },
  "run": {
    "method": "POST",
    "path": "/api/v1/farm/runs"
  },
  "runs": {
    "method": "GET",
    "path": "/api/v1/farm/runs"
  },
  "sample": {
    "method": "GET",
    "path": "/api/v1/traffic/samples/{id}"
  },
  "services": {
    "method": "GET",
    "path": "/api/v1/services"
  },
  "sessions": {
    "method": "GET",
    "path": "/api/v1/auth/sessions"
  },
  "settings": {
    "method": "GET",
    "path": "/api/v1/system/settings"
  },
  "stream": {
    "method": "GET",
    "path": "/api/v1/traffic/events/stream"
  },
  "submissions": {
    "method": "GET",
    "path": "/api/v1/submissions"
  },
  "submit": {
    "method": "POST",
    "path": "/api/v1/flags/{id}/submit"
  },
  "submitter_batch": {
    "method": "GET",
    "path": "/api/v1/submitter/batches/{id}"
  },
  "submitter_batches": {
    "method": "GET",
    "path": "/api/v1/submitter/batches"
  },
  "submitter_config": {
    "method": "GET",
    "path": "/api/v1/submitter"
  },
  "submitter_configure": {
    "method": "PUT",
    "path": "/api/v1/submitter"
  },
  "submitter_run": {
    "method": "POST",
    "path": "/api/v1/submitter/run"
  },
  "target": {
    "method": "PUT",
    "path": "/api/v1/farm/targets"
  },
  "targets": {
    "method": "GET",
    "path": "/api/v1/farm/targets"
  },
  "test_filter": {
    "method": "POST",
    "path": "/api/v1/filters/{id}/test"
  },
  "test_protocol": {
    "method": "POST",
    "path": "/api/v1/protocols/{id}/{version}/test"
  },
  "triage": {
    "method": "POST",
    "path": "/api/v1/flags/{id}/triage"
  },
  "update_filter": {
    "method": "PATCH",
    "path": "/api/v1/filters/{id}"
  },
  "update_service": {
    "method": "PATCH",
    "path": "/api/v1/services/{id}"
  },
  "update_user": {
    "method": "PATCH",
    "path": "/api/v1/users/{id}"
  },
  "upload": {
    "method": "PUT",
    "path": "/api/v1/protocols/{id}/{version}"
  },
  "versions": {
    "method": "GET",
    "path": "/api/v1/farm/exploits/{id}/versions"
  }
} as const;
export function path(id: keyof typeof operations, values: Record<string, string | number> = {}): string {
  const operation=operations[id];if(!operation)throw Error('Unknown API operation '+id);
  return operation.path.replace(/\{([^}]+)\}/g,(_,name)=>{
    if(values[name]===undefined)throw Error('Missing path parameter '+name);
    return encodeURIComponent(values[name]);
  });
}
