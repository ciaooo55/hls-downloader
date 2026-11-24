use hls_client::{CoreEvent, EventEnvelope, TaskSnapshot};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Default)]
pub struct TaskList {
    pub tasks: BTreeMap<String, TaskSnapshot>,
    versions: HashMap<String, u64>,
    snapshot_sequence: u64,
    pub visible: Vec<String>,
    visible_set: HashSet<String>,
    pub selected: HashSet<String>,
    pub anchor: Option<String>,
    pub filter: String,
    pub query: String,
    pub sort: String,
    pub view_revision: u64,
}

pub struct TaskChange {
    pub id: String,
    pub notice: Option<String>,
}

fn terminal_notice(status: &str) -> Option<&'static str> {
    match status {
        "completed" | "done" => Some("下载完成"),
        "failed" => Some("下载失败"),
        _ => None,
    }
}

impl TaskList {
    pub fn toolbar_targets(&self) -> Vec<&TaskSnapshot> {
        if self.selected.is_empty() {
            let mut tasks: Vec<_> = self.tasks.values().collect();
            tasks.sort_by(|a, b| {
                a.queue_index
                    .cmp(&b.queue_index)
                    .then_with(|| a.task_id.cmp(&b.task_id))
            });
            tasks
        } else {
            self.visible
                .iter()
                .filter(|id| self.selected.contains(*id))
                .filter_map(|id| self.tasks.get(id))
                .collect()
        }
    }

    pub fn replace(&mut self, tasks: Vec<TaskSnapshot>, sequence: u64) {
        self.snapshot_sequence = sequence;
        self.versions = tasks
            .iter()
            .map(|task| (task.task_id.clone(), sequence))
            .collect();
        self.tasks = tasks
            .into_iter()
            .map(|task| (task.task_id.clone(), task))
            .collect();
        self.rebuild();
    }

    pub fn merge_snapshot(&mut self, tasks: Vec<TaskSnapshot>, sequence: u64) {
        if sequence < self.snapshot_sequence {
            return;
        }
        self.snapshot_sequence = sequence;
        let incoming: BTreeMap<_, _> = tasks
            .into_iter()
            .map(|task| (task.task_id.clone(), task))
            .collect();
        self.tasks.retain(|id, _| {
            incoming.contains_key(id) || self.versions.get(id).is_some_and(|v| *v > sequence)
        });
        for (id, task) in incoming {
            if self.versions.get(&id).is_some_and(|v| *v > sequence) {
                continue;
            }
            self.tasks.insert(id.clone(), task);
            self.versions.insert(id, sequence);
        }
        self.versions
            .retain(|id, v| *v > sequence || self.tasks.contains_key(id));
        self.rebuild();
    }

    pub fn apply(&mut self, envelope: &EventEnvelope) -> Option<TaskChange> {
        let (id, snapshot) = match &envelope.event {
            CoreEvent::TaskCreated { snapshot }
            | CoreEvent::TaskUpdated { snapshot }
            | CoreEvent::TaskProgress { snapshot } => (snapshot.task_id.clone(), Some(snapshot)),
            CoreEvent::TaskDeleted { task_id } => (task_id.clone(), None),
            _ => return None,
        };
        if envelope.sequence <= self.snapshot_sequence
            || self
                .versions
                .get(&id)
                .is_some_and(|version| *version >= envelope.sequence)
        {
            return None;
        }
        self.versions.insert(id.clone(), envelope.sequence);
        let mut notice = None;
        if let Some(task) = snapshot {
            if let Some(label) = terminal_notice(&task.status) {
                let previous = self
                    .tasks
                    .get(&id)
                    .and_then(|prev| terminal_notice(&prev.status));
                if previous != Some(label) {
                    let name = [&task.filename, &task.title, &id]
                        .into_iter()
                        .find(|name| !name.is_empty())
                        .unwrap_or(&id);
                    notice = Some(format!("{label}：{name}"));
                }
            }
            self.tasks.insert(id.clone(), task.clone());
        } else {
            self.tasks.remove(&id);
        }
        // Ordinary progress keeps its stable row. Only filter/sort membership
        // changes require a model reorder.
        let was_visible = self.visible_set.contains(&id);
        let is_visible = self.tasks.get(&id).is_some_and(|task| self.matches(task));
        let order_changed = match self.sort.split(':').next().unwrap_or("queue") {
            "size" | "speed" | "eta" | "status" | "progress" => true,
            "name" => !matches!(envelope.event, CoreEvent::TaskProgress { .. }),
            _ => !matches!(envelope.event, CoreEvent::TaskProgress { .. }),
        };
        if was_visible != is_visible || order_changed {
            self.rebuild();
        }
        Some(TaskChange { id, notice })
    }

    pub fn rebuild(&mut self) {
        let mut rows: Vec<_> = self
            .tasks
            .values()
            .filter(|task| self.matches(task))
            .collect();
        let descending = self.sort.ends_with(":desc");
        rows.sort_by(|a, b| {
            let order = match self.sort.split(':').next().unwrap_or("queue") {
                "name" => a.filename.to_lowercase().cmp(&b.filename.to_lowercase()),
                "size" => a.total_bytes.cmp(&b.total_bytes),
                "speed" => a.speed_bytes_per_sec.cmp(&b.speed_bytes_per_sec),
                "progress" => {
                    let ratio =
                        |task: &TaskSnapshot| match task.total_bytes.filter(|total| *total > 0) {
                            Some(total) => (
                                u128::from(task.downloaded_bytes.min(total)),
                                u128::from(total),
                            ),
                            None => (0, 1),
                        };
                    let (done_a, total_a) = ratio(a);
                    let (done_b, total_b) = ratio(b);
                    (done_a * total_b).cmp(&(done_b * total_a))
                }
                "eta" => a.eta_seconds.cmp(&b.eta_seconds),
                "status" => a.status.cmp(&b.status),
                _ => a.queue_index.cmp(&b.queue_index),
            }
            .then_with(|| a.task_id.cmp(&b.task_id));
            if descending {
                order.reverse()
            } else {
                order
            }
        });
        let visible: Vec<_> = rows.into_iter().map(|task| task.task_id.clone()).collect();
        if visible != self.visible {
            self.view_revision = self.view_revision.wrapping_add(1);
        }
        self.visible = visible;
        self.visible_set = self.visible.iter().cloned().collect();
        let visible: HashSet<_> = self.visible.iter().collect();
        self.selected.retain(|id| visible.contains(id));
        if self.anchor.as_ref().is_some_and(|id| !visible.contains(id)) {
            self.anchor = None;
        }
    }

    fn matches(&self, task: &TaskSnapshot) -> bool {
        let filter = match self.filter.as_str() {
            "running" => Self::is_running_status(&task.status),
            "queued" => task.status == "queued",
            "paused" => task.status == "paused",
            "completed" => matches!(task.status.as_str(), "completed" | "done"),
            "failed" => matches!(task.status.as_str(), "failed" | "canceled"),
            value if value.starts_with("queue:") => {
                (if task.queue_id.is_empty() {
                    "default"
                } else {
                    &task.queue_id
                }) == &value[6..]
            }
            value if value.starts_with("category:") => {
                hls_client::download_category(&task.filename, &task.url, task.resource_kind)
                    == &value[9..]
            }
            _ => true,
        };
        filter
            && (self.query.is_empty()
                || format!("{} {} {}", task.filename, task.title, task.url)
                    .to_lowercase()
                    .contains(&self.query.to_lowercase()))
    }

    pub fn is_visible(&self, id: &str) -> bool {
        self.visible_set.contains(id)
    }

    pub fn is_running_status(status: &str) -> bool {
        matches!(
            status,
            "downloading"
                | "recording"
                | "processing"
                | "merging"
                | "remuxing"
                | "finalizing"
                | "checking"
                | "parsing"
                | "probing"
        )
    }

    pub fn nav_counts(&self) -> HashMap<String, i32> {
        let mut counts = HashMap::<String, i32>::new();
        for task in self.tasks.values() {
            *counts.entry("all".into()).or_default() += 1;
            let status = if Self::is_running_status(&task.status) {
                "running"
            } else {
                match task.status.as_str() {
                    "completed" | "done" => "completed",
                    "canceled" | "failed" => "failed",
                    status => status,
                }
            };
            *counts.entry(status.into()).or_default() += 1;
            *counts
                .entry(format!(
                    "category:{}",
                    hls_client::download_category(&task.filename, &task.url, task.resource_kind)
                ))
                .or_default() += 1;
            *counts
                .entry(format!(
                    "queue:{}",
                    if task.queue_id.is_empty() {
                        "default"
                    } else {
                        &task.queue_id
                    }
                ))
                .or_default() += 1;
        }
        counts
    }

    pub fn select(&mut self, id: &str, control: bool, shift: bool) {
        let Some(target) = self.visible.iter().position(|candidate| candidate == id) else {
            return;
        };
        if shift {
            let anchor = self
                .anchor
                .as_ref()
                .and_then(|id| self.visible.iter().position(|candidate| candidate == id))
                .unwrap_or(target);
            if !control {
                self.selected.clear();
            }
            self.selected.extend(
                self.visible[anchor.min(target)..=anchor.max(target)]
                    .iter()
                    .cloned(),
            );
        } else if control {
            if !self.selected.remove(id) {
                self.selected.insert(id.to_owned());
            }
            self.anchor = Some(id.to_owned());
        } else {
            self.selected = HashSet::from([id.to_owned()]);
            self.anchor = Some(id.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_snapshot_preserves_newer_updates_creations_and_deletions() {
        let mut list = TaskList::default();
        list.replace(vec![task("a", 0), task("b", 1)], 10);
        list.apply(&EventEnvelope {
            sequence: 11,
            event: CoreEvent::TaskDeleted {
                task_id: "a".into(),
            },
        });
        let mut paused = task("b", 1);
        paused.status = "paused".into();
        list.apply(&EventEnvelope {
            sequence: 12,
            event: CoreEvent::TaskUpdated { snapshot: paused },
        });
        list.apply(&EventEnvelope {
            sequence: 13,
            event: CoreEvent::TaskCreated {
                snapshot: task("c", 2),
            },
        });
        list.merge_snapshot(vec![task("a", 0), task("b", 1)], 10);
        assert!(!list.tasks.contains_key("a"));
        assert_eq!(list.tasks["b"].status, "paused");
        assert!(list.tasks.contains_key("c"));
        assert_eq!(list.versions["a"], 11);
    }

    #[test]
    fn newer_snapshot_removes_old_tasks_and_prunes_tombstones() {
        let mut list = TaskList::default();
        list.replace(vec![task("a", 0), task("b", 1)], 10);
        list.apply(&EventEnvelope {
            sequence: 11,
            event: CoreEvent::TaskDeleted {
                task_id: "a".into(),
            },
        });
        list.merge_snapshot(vec![task("d", 0)], 15);
        assert_eq!(list.visible, ["d"]);
        assert_eq!(list.versions, HashMap::from([("d".into(), 15)]));
        list.merge_snapshot(vec![task("a", 0)], 10);
        assert_eq!(list.snapshot_sequence, 15);
        assert!(list.tasks.contains_key("d"));
        assert!(!list.tasks.contains_key("a"));
        assert_eq!(list.visible, ["d"]);
    }

    #[test]
    fn replace_resets_session_versions_after_core_restart() {
        let mut list = TaskList::default();
        list.replace(vec![task("old", 0)], 100);
        list.apply(&EventEnvelope {
            sequence: 101,
            event: CoreEvent::TaskDeleted {
                task_id: "old".into(),
            },
        });
        list.replace(vec![task("x", 0)], 1);
        assert_eq!(list.snapshot_sequence, 1);
        assert_eq!(list.versions, HashMap::from([("x".into(), 1)]));
        assert_eq!(list.visible, ["x"]);
        assert!(list
            .apply(&EventEnvelope {
                sequence: 2,
                event: CoreEvent::TaskCreated {
                    snapshot: task("old", 1)
                },
            })
            .is_some());
    }

    fn task(id: &str, index: i64) -> TaskSnapshot {
        TaskSnapshot {
            task_id: id.into(),
            filename: format!("{id}.mp4"),
            queue_index: index,
            status: "downloading".into(),
            ..Default::default()
        }
    }
    #[test]
    fn selection_is_stable_and_pruned_after_filtering() {
        let mut list = TaskList::default();
        list.replace(vec![task("a", 0), task("b", 1), task("c", 2)], 5);
        list.select("a", false, false);
        list.select("c", false, true);
        assert_eq!(list.selected.len(), 3);
        list.select("b", true, false);
        assert_eq!(list.selected.len(), 2);
        list.filter = "completed".into();
        list.rebuild();
        assert!(list.selected.is_empty());
    }
    #[test]
    fn toolbar_targets_include_hidden_tasks_only_without_selection() {
        let mut pending = task("pending", 0);
        pending.status = "paused".into();
        let mut completed = task("completed", 1);
        completed.status = "completed".into();
        let mut list = TaskList {
            filter: "completed".into(),
            ..TaskList::default()
        };
        list.replace(vec![completed, pending], 5);
        assert_eq!(list.visible, vec!["completed"]);
        assert_eq!(
            list.toolbar_targets()
                .iter()
                .map(|task| task.task_id.as_str())
                .collect::<Vec<_>>(),
            vec!["pending", "completed"]
        );
        list.select("completed", false, false);
        assert_eq!(
            list.toolbar_targets()
                .iter()
                .map(|task| task.task_id.as_str())
                .collect::<Vec<_>>(),
            vec!["completed"]
        );
    }
    #[test]
    fn default_queue_includes_legacy_empty_ids_without_matching_named_queues() {
        let legacy = task("legacy", 0);
        let mut explicit = task("explicit", 1);
        explicit.queue_id = "default".into();
        let mut named = task("named", 2);
        named.queue_id = "video".into();
        let mut list = TaskList {
            filter: "queue:default".into(),
            ..TaskList::default()
        };
        list.replace(vec![legacy, explicit, named], 5);
        assert_eq!(list.visible, vec!["legacy", "explicit"]);
        list.filter = "queue:video".into();
        list.rebuild();
        assert_eq!(list.visible, vec!["named"]);
    }
    #[test]
    fn late_command_reply_cannot_resurrect_a_deleted_row() {
        let mut list = TaskList::default();
        list.replace(vec![task("a", 0)], 5);
        list.apply(&EventEnvelope {
            sequence: 7,
            event: CoreEvent::TaskDeleted {
                task_id: "a".into(),
            },
        });
        list.apply(&EventEnvelope {
            sequence: 6,
            event: CoreEvent::TaskProgress {
                snapshot: task("a", 0),
            },
        });
        assert!(list.tasks.is_empty());
        assert!(list.visible.is_empty());
        // A replacement snapshot also supersedes replies for rows it omits.
        list.replace(Vec::new(), 9);
        list.apply(&EventEnvelope {
            sequence: 8,
            event: CoreEvent::TaskUpdated {
                snapshot: task("b", 1),
            },
        });
        assert!(list.tasks.is_empty());
    }

    #[test]
    fn task_notices_follow_new_terminal_transitions_only() {
        let mut list = TaskList::default();
        let mut snapshot = task("a", 0);
        list.replace(vec![snapshot.clone()], 5);
        let mut update = |sequence, status: &str| {
            snapshot.status = status.into();
            list.apply(&EventEnvelope {
                sequence,
                event: CoreEvent::TaskUpdated {
                    snapshot: snapshot.clone(),
                },
            })
            .and_then(|change| change.notice)
        };
        assert_eq!(update(5, "completed"), None);
        assert_eq!(update(6, "completed").as_deref(), Some("下载完成：a.mp4"));
        assert_eq!(update(7, "done"), None);
        assert_eq!(update(6, "failed"), None);
        assert_eq!(update(8, "downloading"), None);
        assert_eq!(update(9, "failed").as_deref(), Some("下载失败：a.mp4"));
        assert_eq!(update(10, "failed"), None);
        assert_eq!(update(11, "canceled"), None);
    }

    #[test]
    fn persisted_ascending_sort_reorders_when_progress_changes() {
        let mut a = task("a", 0);
        a.total_bytes = Some(100);
        a.downloaded_bytes = 80;
        a.speed_bytes_per_sec = 10;
        let mut b = task("b", 1);
        b.total_bytes = Some(1000);
        b.downloaded_bytes = 100;
        b.speed_bytes_per_sec = 20;
        let mut list = TaskList {
            sort: "progress:asc".into(),
            ..TaskList::default()
        };
        list.replace(vec![a.clone(), b.clone()], 5);
        assert_eq!(list.visible, vec!["b", "a"]);
        b.downloaded_bytes = 900;
        list.apply(&EventEnvelope {
            sequence: 6,
            event: CoreEvent::TaskProgress { snapshot: b },
        });
        assert_eq!(list.visible, vec!["a", "b"]);
        list.sort = "speed:asc".into();
        list.rebuild();
        a.speed_bytes_per_sec = 30;
        list.apply(&EventEnvelope {
            sequence: 7,
            event: CoreEvent::TaskProgress { snapshot: a },
        });
        assert_eq!(list.visible, vec!["b", "a"]);
    }

    #[test]
    fn running_filter_and_counts_include_recording_checking_and_parsing() {
        let mut downloading = task("a-dl", 0);
        downloading.status = "downloading".into();
        let mut recording = task("b-rec", 1);
        recording.status = "recording".into();
        let mut checking = task("c-chk", 2);
        checking.status = "checking".into();
        let mut parsing = task("d-prs", 3);
        parsing.status = "parsing".into();
        let mut paused = task("e-psd", 4);
        paused.status = "paused".into();
        let mut list = TaskList {
            filter: "running".into(),
            ..TaskList::default()
        };
        list.replace(vec![downloading, recording, checking, parsing, paused], 5);
        assert_eq!(list.visible, vec!["a-dl", "b-rec", "c-chk", "d-prs"]);
        assert_eq!(list.nav_counts().get("running").copied(), Some(4));
    }
}
