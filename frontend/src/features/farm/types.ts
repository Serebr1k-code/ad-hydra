// These read endpoints expose Object in OpenAPI; shapes follow farm/http.rs.
export interface FarmPage<T> { items: T[]; next_cursor: string | null }
export interface FarmRun { id: string; exploit_id: string; status: string; created_at: number; initiated_by: string; version: string }
export type FarmKind = 'targets' | 'exploits' | 'runs'
