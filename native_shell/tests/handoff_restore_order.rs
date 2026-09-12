use hls_native_shell::CoreStore;

#[test]
fn handoffs_with_equal_timestamps_restore_in_insert_order() {
    let mut store = CoreStore::in_memory().unwrap();
    for id in ["media-push-first", "media-push-second", "media-push-third"] {
        let json = serde_json::json!({
            "id": id,
            "status": "pending",
            "created_at_ms": 42,
        })
        .to_string();
        store.save_handoff(id, &json, "pending", None, 42).unwrap();
    }

    let ids = store
        .load_handoffs()
        .unwrap()
        .into_iter()
        .map(|encoded| {
            serde_json::from_str::<serde_json::Value>(&encoded).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();

    assert_eq!(
        ids,
        vec!["media-push-first", "media-push-second", "media-push-third"]
    );
}
