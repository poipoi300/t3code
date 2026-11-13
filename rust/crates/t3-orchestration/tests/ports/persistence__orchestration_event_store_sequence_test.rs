use t3_contracts::base_schemas::*;
use t3_orchestration::{
    error::Result,
    stores::event_store::{EventFilter, EventStore},
    testing::{TestDatabase, detached_event, project_event},
};

#[tokio::test]
async fn asynchronous_pages_preserve_mixed_scopes_and_command_boundaries() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let (application_sequences, command_sequences, exact_sequences) = fixture.stores.transaction(|tx| {
        let store = EventStore::new();
        let mut application = Vec::new();
        let mut command = Vec::new();
        let mut exact = Vec::new();
        for index in 0..1560 {
            if index % 3 == 1 {
                let project = store.append_project(tx, &project_event(&format!("project:{index}"), &format!("project:{index}"))?)?;
                application.push(serde_json::to_value(project)?["sequence"].as_u64().expect("sequence"));
            } else {
                let selected = index % 5 == 0;
                let row = store.append(tx, Some(&CommandId(if selected { "selected" } else { "other" }.into())), &[detached_event(&format!("event:{index}"), if index % 2 == 0 { "target" } else { "other" })?])?.remove(0);
                if index % 3 == 0 {
                    tx.execute("UPDATE orchestration_events SET application_event_version=1 WHERE sequence=?1", [row.sequence as i64])?;
                } else {
                    application.push(row.sequence);
                    if selected && index % 2 == 0 && row.sequence > 26 && row.sequence <= 1526 { command.push(row.sequence); }
                }
            }
        }
        for index in 0..1001 {
            store.append(tx, Some(&CommandId("many".into())), &[detached_event(&format!("many:{index}"), "target")?])?;
        }
        for index in 0..500 {
            exact.push(store.append(tx, Some(&CommandId("exact".into())), &[detached_event(&format!("exact:{index}"), "target")?])?.remove(0).sequence);
        }
        application.retain(|sequence| *sequence > 26 && *sequence <= 1526);
        Ok((application, command, exact))
    }).await?;
    let store = EventStore::new();
    let mut pages = store.read_application_paged(
        fixture.stores.clone(),
        EventFilter {
            after_sequence: 26,
            through_sequence: Some(1526),
            ..Default::default()
        },
    );
    let mut actual = Vec::new();
    while let Some(page) = pages.next_page().await? {
        assert!(page.len() <= 500);
        actual.extend(page.into_iter().map(|event| {
            match event {
                t3_contracts::application_event::ApplicationStoredEvent::Project(event) => {
                    serde_json::to_value(event).expect("project")["sequence"]
                        .as_u64()
                        .expect("sequence")
                }
                t3_contracts::application_event::ApplicationStoredEvent::Orchestration(event) => {
                    event.sequence
                }
            }
        }));
    }
    assert_eq!(actual, application_sequences);
    let mut pages = store.read_paged(
        fixture.stores.clone(),
        EventFilter {
            after_sequence: 26,
            through_sequence: Some(1526),
            thread_id: Some(ThreadId("target".into())),
            command_id: Some(CommandId("selected".into())),
            limit: Some(17),
            ..Default::default()
        },
    );
    let page = pages.next_page().await?.expect("scoped page");
    assert_eq!(
        page.iter().map(|event| event.sequence).collect::<Vec<_>>(),
        command_sequences.into_iter().take(17).collect::<Vec<_>>()
    );
    assert!(pages.next_page().await?.is_none());
    for (command, expected_lengths) in [
        ("many", vec![500, 500, 1]),
        ("exact", vec![500]),
        ("missing", vec![]),
    ] {
        let mut pages = store.read_paged(
            fixture.stores.clone(),
            EventFilter {
                command_id: Some(CommandId(command.into())),
                ..Default::default()
            },
        );
        let mut lengths = Vec::new();
        let mut sequences = Vec::new();
        while let Some(page) = pages.next_page().await? {
            lengths.push(page.len());
            sequences.extend(page.into_iter().map(|event| event.sequence));
        }
        assert_eq!(lengths, expected_lengths);
        if command == "exact" {
            assert_eq!(sequences, exact_sequences);
        }
    }
    for limit in [0, -1] {
        let mut pages = store.read_paged(
            fixture.stores.clone(),
            EventFilter {
                limit: Some(limit),
                ..Default::default()
            },
        );
        assert!(pages.next_page().await?.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn paged_reads_are_lazy_and_continue_on_the_reader_while_the_writer_is_occupied() -> Result<()>
{
    let fixture = TestDatabase::temporary()?;
    fixture
        .stores
        .transaction(|tx| {
            let store = EventStore::new();
            for index in 0..501 {
                store.append(
                    tx,
                    None,
                    &[detached_event(&format!("event:{index}"), "thread")?],
                )?;
            }
            tx.execute(
                "UPDATE orchestration_events SET payload_json='{}' WHERE sequence=501",
                [],
            )?;
            Ok(())
        })
        .await?;
    let mut pages = EventStore::new().read_paged(fixture.stores.clone(), EventFilter::default());
    let (started, wait_started) = tokio::sync::oneshot::channel();
    let (release, wait_release) = std::sync::mpsc::channel();
    let writer = fixture.stores.clone();
    let task = tokio::spawn(async move {
        writer
            .transaction(move |_tx| {
                let _ = started.send(());
                let _ = wait_release.recv();
                Ok(())
            })
            .await
    });
    wait_started.await.expect("writer started");
    let first = pages.next_page().await;
    let _ = release.send(());
    task.await.expect("writer joined")?;
    assert_eq!(first?.expect("first page").len(), 500);
    assert!(pages.next_page().await.is_err());
    assert!(pages.next_page().await?.is_none());
    fixture
        .stores
        .read(|db| {
            assert_eq!(EventStore::new().latest_agent_sequence(db, None)?, 501);
            Ok(())
        })
        .await?;
    Ok(())
}
#[tokio::test]
async fn application_and_scoped_high_waters_exclude_legacy_and_future_thread_versions() -> Result<()>
{
    let fixture = TestDatabase::memory()?;
    fixture.stores.transaction(|tx| {
        let store=EventStore::new();assert_eq!(store.latest_application_sequence(tx)?,0);assert_eq!(store.latest_agent_sequence(tx,None)?,0);
        store.append(tx,None,&[detached_event("legacy","legacy")?])?;
        tx.execute("UPDATE orchestration_events SET application_event_version=1",[])?;
        assert_eq!(store.latest_application_sequence(tx)?,0);
        store.append_project(tx,&project_event("project","project")?)?;
        tx.execute("UPDATE orchestration_events SET application_event_version=1 WHERE event_id='project'",[])?;
        store.append(tx,None,&[detached_event("target","target")?,detached_event("other","other")?])?;
        assert_eq!(store.latest_application_sequence(tx)?,4);assert_eq!(store.latest_agent_sequence(tx,Some(&ThreadId("target".into())))?,3);
        store.append(tx,None,&[detached_event("future","target")?])?;
        tx.execute("UPDATE orchestration_events SET application_event_version=3 WHERE event_id='future'",[])?;
        assert_eq!(store.latest_agent_sequence(tx,None)?,4);
        assert_eq!(store.latest_agent_sequence(tx,Some(&ThreadId("missing".into())))?,0);
        tx.execute("DELETE FROM orchestration_events WHERE sequence>2",[])?;
        assert_eq!(store.latest_application_sequence(tx)?,2);assert_eq!(store.latest_agent_sequence(tx,None)?,0);
        Ok(())
    }).await
}
#[tokio::test]
async fn reads_all_pages_and_applies_sequence_stream_command_and_limit_filters() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture
        .stores
        .transaction(|tx| {
            let store = EventStore::new();
            let command = CommandId("selected".into());
            for i in 0..1560 {
                if i % 3 == 1 {
                    store.append_project(
                        tx,
                        &project_event(&format!("project-{i}"), &format!("project-{i}"))?,
                    )?;
                } else {
                    store.append(
                        tx,
                        Some(&command),
                        &[detached_event(
                            &format!("event-{i}"),
                            if i % 2 == 0 { "target" } else { "other" },
                        )?],
                    )?;
                }
            }
            assert_eq!(
                store
                    .read_application(
                        tx,
                        &EventFilter {
                            after_sequence: 25,
                            through_sequence: Some(1525),
                            ..Default::default()
                        }
                    )?
                    .len(),
                1500
            );
            let filter = EventFilter {
                after_sequence: 25,
                through_sequence: Some(1525),
                thread_id: Some(ThreadId("target".into())),
                command_id: Some(command),
                limit: Some(17),
                ..Default::default()
            };
            let rows = store.read(tx, &filter)?;
            assert_eq!(rows.len(), 17);
            assert!(store.read(tx, &EventFilter { event_type: Some("run.created".into()), ..filter.clone() })?.is_empty());
            assert_eq!(store.read(tx, &EventFilter { event_type: Some("provider-session.detached".into()), ..filter })?, rows);
            assert!(rows.windows(2).all(|w| w[0].sequence < w[1].sequence));
            assert!(
                store
                    .read(
                        tx,
                        &EventFilter {
                            limit: Some(0),
                            ..Default::default()
                        }
                    )?
                    .is_empty()
            );
            assert_eq!(store.pruned_through_sequence(tx)?, 0);
            tx.execute("INSERT INTO orchestration_v2_projection_metadata(projection_name,schema_version,last_sequence,updated_at) VALUES('event-log-pruned-through',1,25,'2026-09-03T00:00:00.000Z')", [])?;
            assert_eq!(store.pruned_through_sequence(tx)?, 25);
            Ok(())
        })
        .await
}
