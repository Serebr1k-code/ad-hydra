export interface FarmPage<T> { items: T[]; next_cursor: string | null }
export interface FarmRun {
  id: string; exploit_id: string; status: string; created_at: number; initiated_by: string; version: string
  started_at?: number | null; finished_at?: number | null; exit_code?: number | null
}
export interface FarmRunDetail extends FarmRun {
  stdout: string; stderr: string; service_id?: string | null; team_ids?: string[]; target_ids?: string[]
  stdout_truncated?: boolean; stderr_truncated?: boolean
}
