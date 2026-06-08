//! EventLog edge tests — EventStore dedup, SnapshotStore frequency, ReadModel rebuild, WAL.

#[cfg(test)]
mod event_log_edge_tests {
    use crate::log::event_log::{
        Event, EventStore, Snapshot, SnapshotStore, ReadModel,
    };

    #[test]
    fn test_event_store_empty_initially() {
        let store = EventStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_event_store_append_and_retrieve() {
        let mut store = EventStore::new();
        let agg_id = uuid::Uuid::now_v7();
        let event = Event::new("AccountCreated", agg_id, r#"{"balance":0}"#, 1);
        store.append(event, uuid::Uuid::now_v7());
        assert!(!store.is_empty());
        assert_eq!(store.len(), 1);
        assert_eq!(store.events_for_aggregate(agg_id).len(), 1);
    }

    #[test]
    fn test_event_store_dedup_command() {
        let mut store = EventStore::new();
        let agg_id = uuid::Uuid::now_v7();
        let cmd_id = uuid::Uuid::now_v7();
        let event1 = Event::new("AccountCreated", agg_id, "{}", 1);
        store.append(event1, cmd_id);
        assert!(store.is_duplicate(cmd_id), "is_duplicate should detect repeat command");
        // Note: append does not auto-dedup; caller must check is_duplicate first
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_event_store_multiple_aggregates() {
        let mut store = EventStore::new();
        let a1 = uuid::Uuid::now_v7();
        let a2 = uuid::Uuid::now_v7();
        store.append(Event::new("Created", a1, "{}", 1), uuid::Uuid::now_v7());
        store.append(Event::new("Created", a2, "{}", 1), uuid::Uuid::now_v7());
        store.append(Event::new("Credited", a1, "{}", 2), uuid::Uuid::now_v7());
        assert_eq!(store.len(), 3);
        assert_eq!(store.events_for_aggregate(a1).len(), 2);
        assert_eq!(store.events_for_aggregate(a2).len(), 1);
    }

    #[test]
    fn test_snapshot_store_frequency() {
        let store = SnapshotStore::new(10);
        assert!(!store.should_snapshot(1));
        assert!(store.should_snapshot(10));
        assert!(store.should_snapshot(20));
        assert!(!store.should_snapshot(15));
    }

    #[test]
    fn test_snapshot_store_save_and_latest() {
        let mut store = SnapshotStore::new(5);
        let agg_id = uuid::Uuid::now_v7();
        let snap = Snapshot {
            aggregate_id: agg_id,
            version: 5,
            state_json: r#"{"balance":1000}"#.to_string(),
            taken_at: chrono::Utc::now(),
        };
        store.save(snap);
        let latest = store.latest(agg_id);
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().version, 5);
    }

    #[test]
    fn test_snapshot_latest_nonexistent() {
        let store = SnapshotStore::new(5);
        assert!(store.latest(uuid::Uuid::now_v7()).is_none());
    }

    #[test]
    fn test_read_model_new_and_apply() {
        let mut model = ReadModel::new();
        let event = Event::new("AccountCreated", uuid::Uuid::now_v7(), r#"{"balance":500}"#, 1);
        model.apply(&event);
    }

    #[test]
    fn test_read_model_rebuild_from_events() {
        let agg = uuid::Uuid::now_v7();
        let events = vec![
            Event::new("Created", agg, r#"{"balance":0}"#, 1),
            Event::new("Credited", agg, r#"{"amount":100}"#, 2),
        ];
        let _model = ReadModel::rebuild(&events);
    }

    #[test]
    fn test_wal_create_and_append() {
        let dir = std::env::temp_dir();
        let wal_path = dir.join(format!("test-{}.wal", uuid::Uuid::now_v7()));

        let mut wal = crate::log::event_log::WriteAheadLog::create(
            wal_path.to_str().unwrap()
        ).unwrap();

        let event = Event::new("TestEvent", uuid::Uuid::now_v7(), "{}", 1);
        let seq = wal.append(event).unwrap();
        assert!(seq >= 1); // sequence counter, first may be 1
        let _ = std::fs::remove_file(&wal_path);
    }

    #[test]
    fn test_wal_replay_after_append() {
        let dir = std::env::temp_dir();
        let wal_path = dir.join(format!("replay-{}.wal", uuid::Uuid::now_v7()));

        {
            let mut wal = crate::log::event_log::WriteAheadLog::create(
                wal_path.to_str().unwrap()
            ).unwrap();
            wal.append(Event::new("E1", uuid::Uuid::now_v7(), "{}", 1)).unwrap();
            wal.append(Event::new("E2", uuid::Uuid::now_v7(), "{}", 2)).unwrap();
        }

        let result = crate::log::event_log::WriteAheadLog::replay(
            wal_path.to_str().unwrap()
        );
        assert!(result.is_ok());
        let replay = result.unwrap();
        assert!(replay.entries.len() >= 1);

        let _ = std::fs::remove_file(&wal_path);
    }
}
