use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EffectivenessQuery {
    pub service_id: Option<String>,
    pub window: Option<String>,
    #[serde(default)]
    pub row_offset: u32,
    #[serde(default)]
    pub team_offset: u32,
}

#[derive(Serialize, ToSchema)]
pub struct EffectivenessRow {
    pub exploit_id: String,
    pub name: String,
    pub service_id: Option<String>,
    pub service_name: String,
}

#[derive(Serialize, ToSchema)]
pub struct EffectivenessTeam {
    /// Namespaced catalog identity; legacy targets and teams remain distinct.
    pub key: String,
    pub id: String,
    pub name: String,
    pub host: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct EffectivenessLatestRun {
    pub id: String,
    pub status: String,
    pub mode: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i32>,
    pub process_status: Option<String>,
    /// Milliseconds since Unix epoch; sandbox attempts include setup.
    pub process_started_at: Option<i64>,
    pub process_finished_at: Option<i64>,
    pub process_exit_code: Option<i32>,
}

#[derive(Serialize, ToSchema)]
pub struct EffectivenessCell {
    pub exploit_id: String,
    pub service_id: Option<String>,
    pub team_key: String,
    pub runs: u32,
    /// Recorded target attempts; excludes shared batch processes and unknown legacy attempts.
    pub target_attempts: u32,
    pub new_flags: u32,
    pub duplicate_flags: u32,
    /// Accepted flags credited exclusively to their original run and target.
    pub accepted_flags: u32,
    /// Capture time of the most recent originally captured flag now accepted.
    pub last_accepted_capture_at: Option<i64>,
    pub flag_counts_complete: bool,
    pub latest: Option<EffectivenessLatestRun>,
}

#[derive(Serialize, ToSchema)]
pub struct EffectivenessMatrix {
    pub rows: Vec<EffectivenessRow>,
    pub teams: Vec<EffectivenessTeam>,
    pub cells: Vec<EffectivenessCell>,
    pub next_row_offset: Option<u32>,
    pub next_team_offset: Option<u32>,
    pub window: String,
    pub since: Option<i64>,
    pub generated_at: i64,
}
