use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CrawlSpec {
    pub entry_url: String,
    pub output_dir: String,
    pub queue_id: String,
    pub same_origin: bool,
    pub max_depth: u32,
    pub max_pages: u32,
    pub page_concurrency: u32,
    pub extensions: Vec<String>,
    pub include_paths: Vec<String>,
    pub exclude_paths: Vec<String>,
    pub offline: bool,
    pub sync_interval_secs: u64,
}
impl Default for CrawlSpec {
    fn default() -> Self {
        Self {
            entry_url: String::new(),
            output_dir: String::new(),
            queue_id: "default".into(),
            same_origin: true,
            max_depth: 2,
            max_pages: 500,
            page_concurrency: 2,
            extensions: Vec::new(),
            include_paths: Vec::new(),
            exclude_paths: Vec::new(),
            offline: false,
            sync_interval_secs: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrawlResource {
    pub url: String,
    pub kind: String,
    pub local_name: String,
    pub selected: bool,
    pub task_id: Option<String>,
    pub saved: bool,
    pub etag: String,
    pub last_modified: String,
    pub last_checked_ms: u64,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrawlProject {
    pub id: String,
    pub spec: CrawlSpec,
    pub status: String,
    pub reason: String,
    pub visited_pages: u32,
    pub resources: Vec<CrawlResource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CrawlRequest {
    Create {
        spec: CrawlSpec,
    },
    List,
    Get {
        project_id: String,
    },
    Pause {
        project_id: String,
    },
    Resume {
        project_id: String,
    },
    Enqueue {
        project_id: String,
        urls: Vec<String>,
    },
    Sync {
        project_id: String,
    },
}
