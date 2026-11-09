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

    pub fn apply(&mut self, envelope: &EventEnvelope) -> Option<String> {
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
        if let Some(task) = snapshot {
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
        Some(id)
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
            "running" => matches!(
                task.status.as_str(),
                "downloading" | "processing" | "merging" | "probing"
            ),
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
}
