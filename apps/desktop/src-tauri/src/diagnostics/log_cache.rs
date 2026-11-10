//! Process-local recent logs. Sequence numbers never reset, including on clear.
use serde::Serialize;
use std::collections::VecDeque;

const MAX_ENTRIES: usize = 2000;
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_PAYLOAD_BYTES: usize = 8192;

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub sequence: u64,
    #[serde(rename = "type")]
    pub level: String,
    pub payload: String,
}

#[derive(Clone, Serialize)]
pub struct Snapshot {
    pub entries: Vec<Entry>,
    pub sequence: u64,
    pub connected: bool,
    pub generation: u64,
}

#[derive(Default)]
pub struct LogCache {
    entries: VecDeque<Entry>,
    bytes: usize,
    sequence: u64,
    generation: u64,
    connected: bool,
}

impl LogCache {
    pub fn begin_stream(&mut self) -> u64 {
        self.generation += 1;
        self.connected = false;
        self.generation
    }

    pub fn set_connected(&mut self, generation: u64, connected: bool) -> Option<Snapshot> {
        if generation != self.generation {
            return None;
        }
        self.connected = connected;
        Some(Snapshot {
            entries: vec![],
            sequence: self.sequence,
            connected,
            generation,
        })
    }

    pub fn append(&mut self, generation: u64, raw: &serde_json::Value) -> Option<Entry> {
        if generation != self.generation {
            return None;
        }
        let payload = raw.get("payload")?.as_str()?;
        if payload.is_empty() {
            return None;
        }
        self.sequence += 1;
        let payload = if payload.len() > MAX_PAYLOAD_BYTES {
            let mut end = MAX_PAYLOAD_BYTES - '…'.len_utf8();
            while !payload.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}…", &payload[..end])
        } else {
            payload.to_owned()
        };
        let entry = Entry {
            sequence: self.sequence,
            level: raw
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("info")
                .chars()
                .take(16)
                .collect(),
            payload,
        };
        self.bytes += entry.level.len() + entry.payload.len();
        self.entries.push_back(entry.clone());
        while self.entries.len() > MAX_ENTRIES || self.bytes > MAX_BYTES {
            if let Some(old) = self.entries.pop_front() {
                self.bytes -= old.level.len() + old.payload.len();
            }
        }
        Some(entry)
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            entries: self.entries.iter().cloned().collect(),
            sequence: self.sequence,
            connected: self.connected,
            generation: self.generation,
        }
    }

    pub fn clear(&mut self) -> Snapshot {
        self.entries.clear();
        self.bytes = 0;
        self.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn raw(payload: &str) -> serde_json::Value {
        serde_json::json!({"type":"info","payload":payload})
    }

    #[test]
    fn history_survives_new_stream_but_rejects_old_reader() {
        let mut cache = LogCache::default();
        let old = cache.begin_stream();
        cache.append(old, &raw("first")).unwrap();
        let current = cache.begin_stream();
        assert!(cache.append(old, &raw("stale")).is_none());
        assert!(cache.set_connected(old, true).is_none());
        cache.set_connected(current, true).unwrap();
        assert_eq!(cache.snapshot().entries[0].payload, "first");
        assert!(cache.snapshot().connected);
    }

    #[test]
    fn clear_keeps_monotonic_cursor_for_late_events() {
        let mut cache = LogCache::default();
        let generation = cache.begin_stream();
        cache.append(generation, &raw("same text")).unwrap();
        let cleared = cache.clear();
        assert!(cleared.entries.is_empty());
        let next = cache.append(generation, &raw("same text")).unwrap();
        assert!(next.sequence > cleared.sequence);
    }

    #[test]
    fn bounds_count_bytes_and_utf8_payload() {
        let mut cache = LogCache::default();
        let generation = cache.begin_stream();
        for _ in 0..MAX_ENTRIES + 10 {
            cache.append(generation, &raw("tiny")).unwrap();
        }
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert_eq!(cache.entries.front().unwrap().sequence, 11);
        for _ in 0..400 {
            cache
                .append(generation, &raw(&"中文".repeat(3000)))
                .unwrap();
        }
        assert!(cache.bytes <= MAX_BYTES);
        assert!(cache.entries.len() < MAX_ENTRIES);
        assert!(cache.entries.back().unwrap().payload.ends_with('…'));
        assert!(cache.entries.back().unwrap().payload.len() <= MAX_PAYLOAD_BYTES);
    }
}
