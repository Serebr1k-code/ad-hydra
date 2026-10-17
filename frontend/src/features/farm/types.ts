export interface FarmPage<T> { items: T[]; next_cursor: string | null }
export interface FarmRun {
  id: string; exploit_id: string; status: string; created_at: number; initiated_by: string; version: string
  exploit_name?:string; service_name?:string; service_id?:string|null; team_ids?:string[]; target_ids?:string[]
  teams?:{id:string;name:string;host:string|null}[]
  new_flags?:number; existing_flags?:number; flag_counts_complete?:boolean
  started_at?: number | null; finished_at?: number | null; exit_code?: number | null
}
export interface FarmRunDetail extends FarmRun {
  stdout: string; stderr: string; service_id?: string | null; team_ids?: string[]; target_ids?: string[]
  flag_counts?: {target_id:string;new_flags:number;existing_flags:number}[]
  stdout_truncated?: boolean; stderr_truncated?: boolean
}

export interface FarmLogPage {stream:string;text:string;start_offset:number;next_offset:number;end_offset:number;done:boolean;dropped_bytes:number;history_lost:boolean}
