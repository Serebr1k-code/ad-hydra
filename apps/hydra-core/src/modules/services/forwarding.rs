use super::Control;
use hydra_domain::{ForwardFraming, Forwarding};
use serde::Deserialize;
use utoipa::ToSchema;
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ForwardingInput {
    pub team_id: String,
    #[schema(minimum = 1, maximum = 65535)]
    pub target_port: u16,
    pub flag_pattern: String,
    #[schema(value_type=Object)]
    pub framing: ForwardFraming,
}
impl ForwardingInput {
    pub async fn resolve(self, control: &Control) -> anyhow::Result<Forwarding> {
        anyhow::ensure!(
            self.target_port > 0 && !self.team_id.is_empty() && self.team_id.len() <= 128,
            "select a forwarding team and target port"
        );
        let team = self.team_id.clone();
        let host: String = control
            .auth
            .db
            .run(move |db| {
                Ok(db.query_row(
                    "SELECT host FROM farm_teams WHERE id=?1 AND enabled=1",
                    [team],
                    |r| r.get(0),
                )?)
            })
            .await
            .map_err(|_| anyhow::anyhow!("forwarding target must be an enabled configured team"))?;
        let ip: std::net::IpAddr = host.parse()?;
        let config = Forwarding {
            team_id: self.team_id,
            target_address: std::net::SocketAddr::new(ip, self.target_port),
            flag_pattern: self.flag_pattern,
            framing: self.framing,
        };
        hydra_rewrite::validate(&config)?;
        Ok(config)
    }
}
