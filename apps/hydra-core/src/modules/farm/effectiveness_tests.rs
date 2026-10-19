use super::effectiveness_dto::{EffectivenessMatrix, EffectivenessQuery};
use super::*;
use crate::modules::{audit::Actor, auth::now};
use rusqlite::params;

async fn fixture() -> (Farm, std::path::PathBuf) {
    let (farm, dir) = super::tests::setup().await;
    farm.auth.db.run(|db| {
        db.execute_batch("INSERT INTO farm_services VALUES('web','Web',80,'tcp',1,1,0,0,'owner'),('other','Other',81,'tcp',1,1,0,0,'owner'); INSERT INTO farm_teams VALUES('one','Renamed one','127.0.0.1',1,1,0,0,'owner'),('two','Team two','127.0.0.2',1,1,0,0,'owner');")?;
        for (id,service) in [("a","web"),("b","web"),("c","other")] {
            db.execute("INSERT INTO farm_exploits(id,name,version,manifest,target_ids,initiated_by,created_at,updated_at,service_id) VALUES(?1,?1,'v','{}','[]','owner',0,0,?2)",params![id,service])?;
        }
        for (id,exploit,service,target,age,status,mode) in [
            ("r1","a","web","one",90,"failed","per_target"),
            ("r2","a","web","one",30,"failed","per_target"),
            ("r3","a","web","two",20,"succeeded","batch"),
            ("old","a","web","one",90000,"timeout","per_target"),
            ("other","c","other","one",10,"running","per_target"),
            ("deleted","deleted-exploit","web","deleted-team",10,"succeeded","batch"),
        ] {
            let snapshot=serde_json::json!([{"id":target,"name":format!("Saved {target}"),"host":"127.0.0.9","port":80,"transport":"tcp","enabled":true,"revision":1}]).to_string();
            db.execute("INSERT INTO farm_runs(id,exploit_id,version,manifest,target_ids,initiated_by,status,created_at,started_at,finished_at,service_id,target_snapshot) VALUES(?1,?2,'v',?3,?4,'owner',?5,?6,?6,?7,?8,?9)",params![id,exploit,serde_json::json!({"mode":mode}).to_string(),serde_json::json!([target]).to_string(),status,now()-age,if status=="running" {None}else{Some(now()-age+1)},service,snapshot])?;
        }
        db.execute_batch("INSERT INTO farm_flag_observations VALUES('r1','one','digest-a',1),('r2','one','digest-a',0),('r2','one','digest-rejected',1),('r3','two','digest-b',1),('old','one','digest-old',1),('other','one','digest-a',0); INSERT INTO farm_flag_stats_legacy_runs VALUES('r1'); INSERT INTO farm_process_results VALUES('r1','one','per_target','succeeded',1000,1050,0),('r2','one','per_target','failed',2000,2500,1),('r3','','batch','succeeded',2000,2600,0);")?;
        for (id,digest,run,target,state) in [("fa","digest-a","r1","one","accepted"),("fb","digest-b","r3","two","accepted"),("fr","digest-rejected","r2","one","failed")] {
            db.execute("INSERT INTO flags(id,digest,category,value,target_id,run_id,initiated_by,created_at) VALUES(?1,?2,'captured','sensitive',?3,?4,'owner',?5)",params![id,digest,target,run,now()-25])?;
            db.execute("INSERT INTO submissions(id,flag_id,state,next_attempt,created_at,initiated_by) VALUES(?1,?1,?2,0,0,'owner')",params![id,state])?;
        }
        Ok(())
    }).await.unwrap();
    (farm, dir)
}
async fn matrix(
    farm: &Farm,
    service: Option<&str>,
    window: &str,
    row_offset: u32,
    team_offset: u32,
) -> EffectivenessMatrix {
    super::effectiveness_repository::read(
        &farm.auth.db,
        Actor::user("reader".into()),
        EffectivenessQuery {
            service_id: service.map(str::to_owned),
            window: None,
            row_offset,
            team_offset,
        },
        window.into(),
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn effectiveness_projects_real_outcomes_without_multiplying_accepted_flags() {
    let (farm, dir) = fixture().await;
    let result = matrix(&farm, Some("web"), "24h", 0, 0).await;
    assert_eq!(
        result
            .rows
            .iter()
            .map(|r| r.exploit_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "deleted-exploit"]
    );
    let one = result
        .cells
        .iter()
        .find(|c| c.exploit_id == "a" && c.team_key == "team:one")
        .unwrap();
    assert_eq!(
        (
            one.runs,
            one.target_attempts,
            one.new_flags,
            one.duplicate_flags,
            one.accepted_flags
        ),
        (2, 2, 2, 1, 1)
    );
    assert!(!one.flag_counts_complete);
    let latest = one.latest.as_ref().unwrap();
    assert_eq!(latest.id, "r2");
    assert_eq!(latest.process_status.as_deref(), Some("failed"));
    assert_eq!(latest.process_exit_code, Some(1));
    assert_eq!(latest.process_finished_at, Some(2500));
    assert!(one.last_accepted_capture_at.is_some());
    let two = result
        .cells
        .iter()
        .find(|c| c.exploit_id == "a" && c.team_key == "team:two")
        .unwrap();
    assert_eq!(
        (two.runs, two.target_attempts, two.accepted_flags),
        (1, 0, 1)
    );
    assert!(two.latest.as_ref().unwrap().process_status.is_none()); // A shared batch exit is not this team's outcome.
    let no_run = result
        .cells
        .iter()
        .find(|c| c.exploit_id == "b" && c.team_key == "team:one")
        .unwrap();
    assert_eq!(no_run.runs, 0);
    assert!(no_run.latest.is_none());
    let deleted = result
        .teams
        .iter()
        .find(|t| t.id == "deleted-team")
        .unwrap();
    assert_eq!(deleted.name, "Saved deleted-team");
    assert_eq!(deleted.host.as_deref(), Some("127.0.0.9"));
    assert_eq!(
        result.teams.iter().find(|t| t.id == "one").unwrap().name,
        "Renamed one"
    );
    let other = matrix(&farm, Some("other"), "24h", 0, 0).await;
    let duplicate_only = other
        .cells
        .iter()
        .find(|c| c.exploit_id == "c" && c.team_key == "team:one")
        .unwrap();
    assert_eq!(
        (
            duplicate_only.new_flags,
            duplicate_only.duplicate_flags,
            duplicate_only.accepted_flags
        ),
        (0, 1, 0)
    );
    let retained = matrix(&farm, Some("web"), "retained", 0, 0).await;
    assert_eq!(
        retained
            .cells
            .iter()
            .find(|c| c.exploit_id == "a" && c.team_key == "team:one")
            .unwrap()
            .runs,
        3
    );
    drop(farm);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn effectiveness_bounds_axes_and_filters_service_before_pagination() {
    let (farm, dir) = fixture().await;
    farm.auth.db.run(|db|{for n in 0..25 {
        db.execute("INSERT INTO farm_exploits(id,name,version,manifest,target_ids,initiated_by,created_at,updated_at,service_id) VALUES(?1,?1,'v','{}','[]','owner',0,0,'other')",[format!("00-other-{n:02}")])?;
        db.execute("INSERT INTO farm_teams VALUES(?1,?1,'127.0.0.1',1,1,0,0,'owner')",[format!("team-{n:02}")])?;
    }Ok(())}).await.unwrap();
    let web = matrix(&farm, Some("web"), "1h", 0, 0).await;
    assert_eq!(web.rows.len(), 3);
    assert!(web.next_row_offset.is_none());
    assert_eq!(web.teams.len(), 10);
    assert_eq!(web.next_team_offset, Some(10));
    assert_eq!(web.cells.len(), 30);
    let other = matrix(&farm, Some("other"), "1h", 0, 0).await;
    assert_eq!(other.rows.len(), 20);
    assert_eq!(other.next_row_offset, Some(20));
    assert_eq!(other.cells.len(), 200);
    let next = matrix(&farm, Some("other"), "1h", 20, 10).await;
    assert_eq!(next.rows.len(), 6);
    assert!(next.next_row_offset.is_none());
    assert_eq!(next.teams.len(), 10);
    assert!(
        other
            .rows
            .iter()
            .all(|r| r.service_id.as_deref() == Some("other"))
    );
    drop(farm);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn effectiveness_http_auth_validation_and_actor_audit() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let (farm, dir) = fixture().await;
    crate::modules::users::create(
        &farm.auth.db,
        &farm.auth.crypto,
        "reader".into(),
        "long-test-password".into(),
    )
    .await
    .unwrap();
    let token = farm
        .auth
        .login("reader".into(), "long-test-password".into(), "test".into())
        .await
        .unwrap()
        .response
        .access_token;
    let app = router(farm.clone());
    assert_eq!(
        app.clone()
            .oneshot(
                Request::get("/api/v1/farm/effectiveness")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
        401
    );
    for query in [
        "window=invalid",
        "row_offset=20001",
        "team_offset=20001",
        "unknown=field",
    ] {
        assert_eq!(
            app.clone()
                .oneshot(
                    Request::get(format!("/api/v1/farm/effectiveness?{query}"))
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap()
                )
                .await
                .unwrap()
                .status(),
            400
        );
    }
    let response = app
        .oneshot(
            Request::get("/api/v1/farm/effectiveness?service_id=web&window=24h")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(!body.contains("sensitive"));
    assert!(!body.contains("digest-"));
    farm.auth.db.run(|db|{assert_eq!(db.query_row("SELECT COUNT(*) FROM audit a JOIN users u ON u.id=a.actor_id WHERE a.action='farm.effectiveness.read' AND u.username='reader'",[],|r|r.get::<_,u32>(0))?,1);Ok(())}).await.unwrap();
    drop(farm);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn recorded_attempts_finish_independently_and_recover_on_restart() {
    let (farm, dir) = fixture().await;
    farm.auth
        .db
        .run(|db| {
            db.execute(
                "UPDATE farm_runs SET status='running',finished_at=NULL WHERE id='other'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    process_results::started(&farm.auth.db, "other", "one", "per_target")
        .await
        .unwrap();
    let run_dir = dir.join("uploads");
    let auth = farm.auth.clone();
    drop(farm);
    let farm = Farm::open(auth, run_dir, std::sync::Arc::new(UnavailableRunner))
        .await
        .unwrap();
    farm.auth.db.run(|db|{
        let (status,start,end):(String,i64,i64)=db.query_row("SELECT status,started_at,finished_at FROM farm_process_results WHERE run_id='other' AND target_id='one'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        assert_eq!(status,"interrupted");assert!(start>1_000_000_000_000);assert!(end>=start);
        assert_eq!(db.query_row("SELECT status FROM farm_runs WHERE id='other'",[],|r|r.get::<_,String>(0))?,"interrupted");
        Ok(())
    }).await.unwrap();
    // A terminal result cannot be changed by a late completion callback.
    process_results::finished(&farm.auth.db, "other", "one", "succeeded", Some(0))
        .await
        .unwrap();
    farm.auth
        .db
        .run(|db| {
            assert_eq!(
                db.query_row(
                    "SELECT status FROM farm_process_results WHERE run_id='other'",
                    [],
                    |r| r.get::<_, String>(0)
                )?,
                "interrupted"
            );
            Ok(())
        })
        .await
        .unwrap();
    drop(farm);
    std::fs::remove_dir_all(dir).unwrap();
}
