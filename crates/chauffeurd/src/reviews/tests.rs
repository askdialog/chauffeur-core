use super::*;

#[test]
fn repeated_report_mentions_and_same_day_events_do_not_inflate_observation_counts() {
    let evidence = |id: &str, from_review| ReviewEvidence {
        id: id.into(),
        workspace_id: "w".into(),
        chat_id: "c".into(),
        workspace: "work".into(),
        branch: "branch".into(),
        at: "2026-11-14T12:00:00Z".into(),
        quote: "failure".into(),
        source_seq: 1,
        from_review,
        role: "assistant_text".into(),
        human: false,
    };
    let input = data::Input {
        seq: 12,
        reports: vec![],
        evidence: HashMap::from([
            ("e1".into(), evidence("e1", false)),
            ("e2".into(), evidence("e2", false)),
            ("e3".into(), evidence("e3", true)),
        ]),
        request: String::new(),
        source_coverage: vec![],
        followed_ids: HashSet::new(),
    };
    let answer = r#"{"overview":"Recurring failure","patterns":[{"id":"ci-check","title":"CI check","summary":"Repeated attempts","trend":"recurring","confidence":"Limited","observation_ids":["e1","e2","e3","made-up"]}],"experiments":[]}"#;
    let p = analysis::parse(answer, &input, None).unwrap();
    assert_eq!(p.patterns[0].observations.len(), 1);
    assert_eq!(p.patterns[0].observations[0].id, "e1");
}

#[test]
fn a_projection_cannot_create_sources_or_claim_an_agent_proposal_was_adopted() {
    let report = ReviewReport {
        id: "weekly:2026-11-08".into(),
        kind: ReviewCadence::Weekly,
        date: "2026-11-08".into(),
        workspace_id: Some("r".into()),
        chat_id: Some("rc".into()),
        title: "Review".into(),
        status: ReviewReportStatus::Complete,
        archived: false,
        shortened: false,
        summary: Some("CI".into()),
    };
    let input = data::Input {
        seq: 1,
        reports: vec![report],
        evidence: HashMap::new(),
        request: String::new(),
        source_coverage: vec![],
        followed_ids: HashSet::new(),
    };
    let p=analysis::parse(r#"{"overview":"Try earlier validation","patterns":[],"experiments":[{"id":"earlier-validation","title":"Earlier validation","origin_report_id":"weekly:2026-11-08","experiment":"Run checks before review","expected_benefit":"Less rework","success_measure":"Count rework cycles","status":"decided","evidence_ids":["invented"]}]}"#,&input,None).unwrap();
    assert_eq!(
        p.experiments[0].status,
        ReviewRecommendationStatus::Proposed
    );
    assert!(p.experiments[0].evidence.is_empty());
    assert!(analysis::parse("not JSON", &input, Some(&p)).is_err());
    let p2 = analysis::parse(
        r#"{"overview":"Not enough new evidence","experiments":[]}"#,
        &input,
        Some(&p),
    )
    .unwrap();
    assert_eq!(p2.experiments[0].id, p.experiments[0].id);
    assert_eq!(p2.experiments[0].origin_date, "2026-11-08");
}

async fn seed(root: &std::path::Path) -> Arc<Daemon> {
    use chauffeur_protocol::client::{CreateChatParams, CreateWorkspaceParams};
    use chauffeur_protocol::events::ChatEvent;
    let repo = root.join("seed");
    std::fs::create_dir_all(&repo).unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let paths = crate::paths::Paths::new(
        root.join("home"),
        root.join("workspaces"),
        root.join("repos"),
    );
    paths.ensure().unwrap();
    std::fs::write(&paths.settings,format!("linear_enabled=false\ngithub_sources=false\ndefault_repos=['repo']\n[search]\nsemantic=false\n[daily_review]\nenabled=false\n[repos.repo]\npath={:?}\ndefault_branch='main'\n",repo.to_string_lossy())).unwrap();
    let d = Daemon::open(paths).unwrap();
    let fake = root.join("fake-sidecar.py");
    std::fs::write(&fake,r#"#!/usr/bin/env python3
import json,sys,time,pathlib
for line in sys.stdin:
 try:
  q=json.loads(line); mid=q.get('id')
  if mid is None: continue
  if q['method']=='agent.models': result={'models':[]}
  elif q['method']=='agent.ask':
   data=json.loads(q['params']['request']); root=pathlib.Path(__file__).parent
   time.sleep(.15)
   if (root/'bad-answer').exists(): answer='invalid JSON'
   else:
    obs=[e['id'] for e in data['original_observations'] if 'schema-check' in e['quote']]
    report=data['reports'][0]['report']['id']
    answer=json.dumps({'overview':'Validate before opening a review. Four original observations show avoidable rework.','gaps':['This is an isolated test fixture. Missing days are unknown.'],'patterns':[{'id':'late-validation','title':'Validation happens after opening the review','summary':'Run schema-check earlier to avoid CI rework.','trend':'recurring','confidence':'Repeated observations; causality remains uncertain.','observation_ids':obs,'counterexample_ids':[],'gaps':['Not every working day has a report.'],'report_ids':[report]}],'experiments':[{'id':'earlier-schema-check','title':'Run schema-check before opening the review','origin_report_id':report,'pattern_id':'late-validation','experiment':'Run the check before the next three eligible reviews.','expected_benefit':'Fewer CI rework cycles.','success_measure':'Count rework cycles over three reviews; compare with the preceding three.','effort':'Two minutes per review','next_check':None,'status':'proposed','evidence_ids':obs[:1]}]})
   result={'text':answer}
  else: result={}
  print(json.dumps({'jsonrpc':'2.0','id':mid,'result':result}),flush=True)
 except Exception as e: print(json.dumps({'jsonrpc':'2.0','id':q.get('id'),'error':{'code':-32603,'message':str(e)}}),flush=True)
"#).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    d.chats.sidecar.use_binary(fake);
    let source = d
        .workspaces
        .create(CreateWorkspaceParams {
            name: Some("schema-validation".into()),
            explore: true,
            ..Default::default()
        })
        .await
        .unwrap()
        .workspace;
    let chat = d
        .chats
        .create(CreateChatParams {
            workspace: source.id.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    for offset in [6, 3, 1, 0] {
        let at = crate::daily_review::bounds(Local::now().date_naive() - Days::new(offset))
            .unwrap()
            .0;
        let payload = serde_json::json!({"ts":at,"event":{"scope":"chat","workspace_id":source.id,"chat_id":chat.id,"event":{"type":"user_message","text":"schema-check failed after opening the review; fix the fixture","queued":false}}});
        d.store
            .append_event(
                &at,
                Some(&source.id),
                Some(&chat.id),
                false,
                &payload.to_string(),
            )
            .unwrap();
    }
    for (offset, kind) in [
        (1, crate::daily_review::ReviewKind::Daily),
        (0, crate::daily_review::ReviewKind::Weekly),
    ] {
        let day = (Local::now().date_naive() - Days::new(offset)).to_string();
        let w = d
            .workspaces
            .create_for_review(
                CreateWorkspaceParams {
                    name: Some(format!(
                        "{}-fixture-review",
                        if offset == 1 { "daily" } else { "weekly" }
                    )),
                    explore: true,
                    ..Default::default()
                },
                Some((&day, kind)),
                Some(crate::daily_review::section(&d).unwrap()),
            )
            .await
            .unwrap()
            .workspace;
        let c = d
            .chats
            .create(CreateChatParams {
                workspace: w.id.clone(),
                ..Default::default()
            })
            .await
            .unwrap();
        d.events.chat(&w.id,&c.id,ChatEvent::AssistantText{message_id:format!("finding-{offset}"),text:"schema-check failed after opening reviews on four separate days. Try running the check before the next three reviews, and measure CI rework cycles.".into(),parent_tool_use_id:None});
        d.store.with(|db|db.execute("INSERT INTO events(ts,workspace_id,chat_id,registry,payload) VALUES (?1,?2,?3,0,?4)",rusqlite::params![now(),w.id,c.id,serde_json::json!({"ts":now(),"event":{"scope":"chat","workspace_id":w.id,"chat_id":c.id,"event":{"type":"turn_ended","outcome":"success"}}}).to_string()])).unwrap();
        if offset == 1 {
            d.chats.close(&c.id, true).await.unwrap();
            d.workspaces.archive(&w.id, true).await.unwrap();
        }
    }
    d
}

async fn wait(d: &Daemon) -> ReviewsInfo {
    for _ in 0..100 {
        let result = info(d).unwrap();
        if !result.running {
            return result;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("review projection did not finish");
}

#[tokio::test]
async fn real_reports_refresh_with_grounded_sources_and_failed_refresh_keeps_previous_insights() {
    let dir = tempfile::tempdir().unwrap();
    let d = seed(dir.path()).await;
    let initial = info(&d).unwrap();
    assert_eq!(initial.reports.len(), 2);
    assert!(initial.reports.iter().any(|r| r.archived));
    assert!(refresh(&d).unwrap().running);
    assert!(
        refresh(&d).unwrap().running,
        "a second click does not start a second projection"
    );
    let first = wait(&d).await;
    assert!(first.last_error.is_none(), "{:?}", first.last_error);
    let p = first.projection.unwrap();
    assert_eq!(p.patterns[0].observations.len(), 4);
    assert!(!first.stale);
    assert!(p.patterns[0].observations.iter().all(|e| !e.from_review));
    let mut expired = p.clone();
    expired.source_coverage.last_mut().unwrap().date =
        (Local::now().date_naive() - Days::new(1)).to_string();
    d.store
        .kv_set(
            "reviews.projection",
            &serde_json::to_string(&expired).unwrap(),
        )
        .unwrap();
    assert!(
        info(&d).unwrap().stale,
        "the calendar window changes even when no turn has finished"
    );
    d.store
        .kv_set("reviews.projection", &serde_json::to_string(&p).unwrap())
        .unwrap();
    std::fs::write(dir.path().join("bad-answer"), "").unwrap();
    refresh(&d).unwrap();
    let failed = wait(&d).await;
    assert!(failed.last_error.is_some());
    assert_eq!(failed.projection.unwrap().generated_at, p.generated_at);
    d.chats.sidecar.shutdown().await;
}

#[tokio::test]
async fn user_followups_require_evidence_for_results_are_idempotent_and_survive_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let d = seed(dir.path()).await;
    refresh(&d).unwrap();
    let initial = wait(&d).await;
    let e = &initial.projection.as_ref().unwrap().experiments[0];
    let mut params = ReviewFollowUpParams {
        experiment_id: e.id.clone(),
        status: Some(ReviewRecommendationStatus::ObservedEffect),
        note: "Three reviews on Nov 14 needed one cycle each, compared with three earlier.".into(),
        source_seq: None,
        idempotency_key: uuid::Uuid::now_v7().to_string(),
    };
    assert!(follow_up(&d, params.clone()).is_err());
    params.source_seq = Some(e.evidence[0].source_seq);
    let saved = follow_up(&d, params.clone()).unwrap();
    assert_eq!(saved.followups.len(), 1);
    assert_eq!(follow_up(&d, params.clone()).unwrap().followups.len(), 1);
    params.note = "A different decision using the same request key".into();
    assert!(follow_up(&d, params).is_err());
    let paths = d.paths.clone();
    d.shutdown.cancel();
    d.chats.sidecar.shutdown().await;
    let next = Daemon::open(paths).unwrap();
    assert_eq!(info(&next).unwrap().followups.len(), 1);
    assert!(info(&next).unwrap().projection.is_some());
}

/// A real daemon with deterministic model output, used only for manual browser verification.
#[tokio::test]
#[ignore = "interactive browser fixture; requires CHAUFFEUR_REVIEWS_FIXTURE_DIR under the temp directory"]
async fn reviews_browser_fixture() {
    let root = std::path::PathBuf::from(std::env::var("CHAUFFEUR_REVIEWS_FIXTURE_DIR").unwrap())
        .canonicalize()
        .unwrap();
    assert!(root.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    assert!(
        root.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("chauffeur-reviews-browser.")
    );
    let d = if root.join("home/chauffeur.db").exists() {
        let paths = crate::paths::Paths::new(
            root.join("home"),
            root.join("workspaces"),
            root.join("repos"),
        );
        let d = Daemon::open(paths).unwrap();
        d.chats.sidecar.use_binary(root.join("fake-sidecar.py"));
        d
    } else {
        seed(&root).await
    };
    refresh(&d).unwrap();
    let result = wait(&d).await;
    assert!(result.last_error.is_none());
    let server = tokio::spawn(crate::server::serve(d.clone()));
    while !root.join("stop").exists() {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    d.shutdown.cancel();
    d.chats.sidecar.shutdown().await;
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn source_sampling_covers_old_days_even_when_today_has_hundreds_of_tool_events() {
    let dir = tempfile::tempdir().unwrap();
    let d = seed(dir.path()).await;
    let initial = data::input(&d).unwrap();
    let source = initial
        .evidence
        .values()
        .find(|e| !e.from_review)
        .unwrap()
        .clone();
    let oldest = Local::now().date_naive() - Days::new(27);
    let ts = crate::daily_review::bounds(oldest).unwrap().0;
    let payload = serde_json::json!({"ts":ts,"event":{"scope":"chat","workspace_id":source.workspace_id,"chat_id":source.chat_id,"event":{"type":"user_message","text":"schema-check failed on the oldest day","queued":false}}});
    let old_seq = d
        .store
        .append_event(
            &ts,
            Some(&source.workspace_id),
            Some(&source.chat_id),
            false,
            &payload.to_string(),
        )
        .unwrap();
    for i in 0..900 {
        d.events.chat(
            &source.workspace_id,
            &source.chat_id,
            chauffeur_protocol::events::ChatEvent::ToolCall {
                tool_use_id: format!("noise-{i}"),
                name: "Read".into(),
                input: serde_json::json!({"path":"src/lib.rs"}),
                parent_tool_use_id: None,
            },
        );
    }
    let sampled = data::input(&d).unwrap();
    assert!(
        sampled.evidence.values().any(|e| e.source_seq == old_seq),
        "old evidence must not be displaced by recent tool noise"
    );
    assert_eq!(sampled.source_coverage.len(), 28);
    let old = &sampled.source_coverage[0];
    assert_eq!(old.date, oldest.to_string());
    assert!(old.complete);
    let today = sampled.source_coverage.last().unwrap();
    assert!(today.available > today.included);
    assert!(!today.complete);
    d.chats.sidecar.shutdown().await;
}

#[tokio::test]
async fn streaming_review_text_does_not_change_the_revision_or_enter_a_projection() {
    use chauffeur_protocol::events::ChatEvent;
    let dir = tempfile::tempdir().unwrap();
    let d = seed(dir.path()).await;
    let before = data::source_seq(&d.store).unwrap();
    let report = info(&d)
        .unwrap()
        .reports
        .into_iter()
        .find(|r| !r.archived)
        .unwrap();
    let chat = report.chat_id.unwrap();
    let w = report.workspace_id.unwrap();
    d.chats.on_event(&chat, ChatEvent::TurnStarted);
    d.events.chat(
        &w,
        &chat,
        ChatEvent::AssistantText {
            message_id: "streaming".into(),
            text: "THIS IS AN UNFINISHED TURN".into(),
            parent_tool_use_id: None,
        },
    );
    assert_eq!(data::source_seq(&d.store).unwrap(), before);
    let input = data::input(&d).unwrap();
    assert!(!input.request.contains("THIS IS AN UNFINISHED TURN"));
    let ts = now();
    let payload = serde_json::json!({"ts":ts,"event":{"scope":"chat","workspace_id":w,"chat_id":chat,"event":{"type":"turn_ended","outcome":"success"}}});
    d.store
        .append_event(&ts, Some(&w), Some(&chat), false, &payload.to_string())
        .unwrap();
    assert!(data::source_seq(&d.store).unwrap() > before);
    assert!(
        data::input(&d)
            .unwrap()
            .request
            .contains("THIS IS AN UNFINISHED TURN")
    );
    d.chats.sidecar.shutdown().await;
}

#[tokio::test]
async fn delegated_guest_and_system_messages_are_not_evidence_of_the_users_decision() {
    let dir = tempfile::tempdir().unwrap();
    let d = seed(dir.path()).await;
    let input = data::input(&d).unwrap();
    let source = input
        .evidence
        .values()
        .find(|e| !e.from_review && e.human)
        .unwrap()
        .clone();
    d.store.with(|c|c.execute("INSERT INTO chats(id,workspace_id,agent,permission_mode,status,created_at,parent_chat) VALUES('delegated',?1,'claude','auto','idle','old',?2)",rusqlite::params![source.workspace_id,source.chat_id])).unwrap();
    for (chat, extra) in [
        (
            &source.chat_id,
            serde_json::json!({"delegated_answer":{"chat_id":"delegated"}}),
        ),
        (
            &source.chat_id,
            serde_json::json!({"suggested_by":{"email":"guest@example.invalid"}}),
        ),
        (&"delegated".to_string(), serde_json::json!({})),
    ] {
        let mut event = serde_json::json!({"type":"user_message","text":"I decided to adopt this recommendation","queued":false});
        event
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let ts = now();
        let payload = serde_json::json!({"ts":ts,"event":{"scope":"chat","workspace_id":source.workspace_id,"chat_id":chat,"event":event}});
        let seq = d
            .store
            .append_event(
                &ts,
                Some(&source.workspace_id),
                Some(chat),
                false,
                &payload.to_string(),
            )
            .unwrap();
        assert!(!data::evidence(&d.store, seq).unwrap().unwrap().human);
    }
    let report = input.reports[0].clone();
    let mut agent = source.clone();
    agent.human = false;
    let mut mapped = data::Input {
        seq: input.seq,
        reports: vec![report.clone()],
        evidence: HashMap::from([(agent.id.clone(), agent)]),
        source_coverage: vec![],
        followed_ids: HashSet::new(),
        request: String::new(),
    };
    let answer = serde_json::json!({"overview":"Try the check","experiments":[{"id":"check","title":"Try the check","origin_report_id":report.id,"experiment":"Run it","expected_benefit":"Less rework","success_measure":"Count cycles","status":"decided","evidence_ids":[source.id]}]});
    assert_eq!(
        analysis::parse(&answer.to_string(), &mapped, None)
            .unwrap()
            .experiments[0]
            .status,
        ReviewRecommendationStatus::Proposed
    );
    mapped.evidence.get_mut(&source.id).unwrap().human = true;
    assert_eq!(
        analysis::parse(&answer.to_string(), &mapped, None)
            .unwrap()
            .experiments[0]
            .status,
        ReviewRecommendationStatus::Decided
    );
    d.chats.sidecar.shutdown().await;
}

#[tokio::test]
async fn note_only_followups_do_not_assert_or_pin_a_recommendation_status() {
    let dir = tempfile::tempdir().unwrap();
    let d = seed(dir.path()).await;
    refresh(&d).unwrap();
    let initial = wait(&d).await;
    let id = initial.projection.unwrap().experiments[0].id.clone();
    let note = ReviewFollowUpParams {
        experiment_id: id,
        status: None,
        note: "Deferred while I investigate the root cause.".into(),
        source_seq: None,
        idempotency_key: uuid::Uuid::now_v7().to_string(),
    };
    let saved = follow_up(&d, note.clone()).unwrap();
    assert_eq!(saved.followups[0].status, None);
    assert_eq!(follow_up(&d, note).unwrap().followups.len(), 1);
    let request = data::input(&d).unwrap().request;
    assert!(request.contains("\"status\":null"));
    d.chats.sidecar.shutdown().await;
}

#[test]
fn model_carryover_is_bounded_but_recorded_followups_keep_their_recommendations() {
    let mut input = data::Input {
        seq: 1,
        reports: vec![],
        evidence: HashMap::new(),
        source_coverage: vec![],
        followed_ids: HashSet::from(["experiment-149".into()]),
        request: String::new(),
    };
    let experiments = (0..150)
        .map(|i| ReviewExperiment {
            id: format!("experiment-{i}"),
            title: format!("Experiment {i}"),
            origin_report_id: "weekly:2026-11-08".into(),
            origin_date: "2026-11-08".into(),
            pattern_id: None,
            experiment: "Try it".into(),
            expected_benefit: "Less rework".into(),
            success_measure: "Count cycles".into(),
            effort: None,
            next_check: None,
            status: ReviewRecommendationStatus::Proposed,
            evidence: vec![],
        })
        .collect();
    let mut previous = ReviewProjection {
        generated_at: now(),
        source_seq: 0,
        overview: "Old projection".into(),
        patterns: vec![],
        experiments,
        gaps: vec![],
        report_ids: vec![],
        source_coverage: vec![],
    };
    let result = analysis::parse(
        r#"{"overview":"No new recommendation"}"#,
        &input,
        Some(&previous),
    )
    .unwrap();
    assert_eq!(result.experiments.len(), 51);
    assert!(result.experiments.iter().any(|e| e.id == "experiment-149"));
    for e in &mut previous.experiments {
        e.status = ReviewRecommendationStatus::ObservedEffect;
    }
    let result = analysis::parse(
        r#"{"overview":"No new recommendation"}"#,
        &input,
        Some(&previous),
    )
    .unwrap();
    assert_eq!(
        result.experiments.len(),
        1,
        "resolved unfollowed suggestions are not retained indefinitely"
    );
    input.followed_ids.clear();
    assert!(
        analysis::parse(
            r#"{"overview":"No new recommendation"}"#,
            &input,
            Some(&previous)
        )
        .unwrap()
        .experiments
        .is_empty()
    );
}
