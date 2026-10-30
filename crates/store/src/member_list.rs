//! Lazy member sidebar (`GUILD_MEMBER_LIST_UPDATE`) state.

use ferrocord_model::gateway::{MemberListGroup, MemberListItem, MemberListOp, MemberListUpdate};

#[derive(Debug, Clone, Default)]
pub struct MemberList {
    pub id: String,
    pub member_count: u32,
    pub online_count: u32,
    pub groups: Vec<MemberListGroup>,
    /// Sparse list of rows; `None` = not yet synced.
    pub rows: Vec<Option<MemberListItem>>,
}

impl MemberList {
    pub fn apply(&mut self, upd: &MemberListUpdate) {
        self.id = upd.id.clone();
        self.member_count = upd.member_count;
        self.online_count = upd.online_count;
        self.groups = upd.groups.clone();
        let total: u32 = upd.groups.iter().map(|g| g.count + 1).sum();
        if self.rows.len() < total as usize {
            self.rows.resize(total as usize, None);
        }
        for op in &upd.ops {
            match op {
                MemberListOp::Sync { range, items } => {
                    let start = range[0] as usize;
                    let need = start + items.len();
                    if self.rows.len() < need {
                        self.rows.resize(need, None);
                    }
                    for (i, item) in items.iter().enumerate() {
                        self.rows[start + i] = Some(item.clone());
                    }
                }
                MemberListOp::Insert { index, item } => {
                    let i = (*index as usize).min(self.rows.len());
                    self.rows.insert(i, Some(item.clone()));
                }
                MemberListOp::Update { index, item } => {
                    let i = *index as usize;
                    if i >= self.rows.len() {
                        self.rows.resize(i + 1, None);
                    }
                    self.rows[i] = Some(item.clone());
                }
                MemberListOp::Delete { index } => {
                    let i = *index as usize;
                    if i < self.rows.len() {
                        self.rows.remove(i);
                    }
                }
                MemberListOp::Invalidate { range } => {
                    let end = (range[1] as usize + 1).min(self.rows.len());
                    for r in self.rows.iter_mut().take(end).skip(range[0] as usize) {
                        *r = None;
                    }
                }
            }
        }
        // Total rows = members + group headers; trim stale tail.
        let total = total as usize;
        if total > 0 && self.rows.len() > total {
            self.rows.truncate(total);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sync_and_ops() {
        let upd: MemberListUpdate = serde_json::from_value(json!({
            "guild_id": "1", "id": "everyone", "member_count": 2, "online_count": 2,
            "groups": [{"id":"online","count":2}],
            "ops": [{"op":"SYNC","range":[0,99],"items":[
                {"group":{"id":"online","count":2}},
                {"member":{"user":{"id":"10","username":"a"},"roles":[],"presence":{"status":"online"}}},
                {"member":{"user":{"id":"11","username":"b"},"roles":[]}}
            ]}]
        }))
        .unwrap();
        let mut l = MemberList::default();
        l.apply(&upd);
        assert_eq!(l.rows.len(), 3);
        let upd2: MemberListUpdate = serde_json::from_value(json!({
            "guild_id": "1", "id": "everyone", "member_count": 1, "online_count": 1,
            "groups": [{"id":"online","count":1}],
            "ops": [{"op":"DELETE","index":2}]
        }))
        .unwrap();
        l.apply(&upd2);
        assert_eq!(l.rows.len(), 2);
    }
}
