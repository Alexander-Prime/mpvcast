use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Default)]
pub struct Queue(Vec<QueueItem>);

impl Queue {
    pub fn replace(&mut self, items: Vec<QueueItem>) {
        self.0 = items;
    }

    pub fn push(&mut self, item: QueueItem) {
        self.0.push(item);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &QueueItem> {
        self.0.iter()
    }

    pub fn get(&self, index: usize) -> Option<&QueueItem> {
        self.0.get(index)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
