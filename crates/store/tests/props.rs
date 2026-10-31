//! Property tests for the store's lazy member list and per-channel message cache.

use ferrocord_model::gateway::{MemberListGroup, MemberListItem, MemberListOp, MemberListUpdate, MessageDelete};
use ferrocord_model::{Event, Message, Snowflake, User};
use ferrocord_store::{MESSAGE_CACHE_LIMIT, MemberList, Store};
use ferrocord_testkit::fixtures::{ReadyBuilder, ids};
use proptest::prelude::*;
use serde_json::{Value, json};

// ---------------------------------------------------------------- member list

fn member(id: u64) -> MemberListItem {
    serde_json::from_value(json!({ "member": { "user": { "id": id.to_string(), "username": format!("u{id}") }, "roles": [] } })).unwrap()
}

fn group(id: &str, count: u32) -> MemberListItem {
    MemberListItem::Group(MemberListGroup { id: id.into(), count })
}

fn row_id(item: &Option<MemberListItem>) -> Option<u64> {
    match item {
        Some(MemberListItem::Member(m)) => m.member.user_id().map(|s| s.0),
        Some(MemberListItem::Group(_)) => Some(0),
        None => None,
    }
}

fn update(groups: Vec<MemberListGroup>, ops: Vec<MemberListOp>) -> MemberListUpdate {
    let member_count = groups.iter().map(|g| g.count).sum();
    MemberListUpdate { guild_id: Snowflake(1), id: "everyone".into(), member_count, online_count: member_count, groups, ops }
}

fn arb_item() -> impl Strategy<Value = MemberListItem> {
    prop_oneof![(1u64..500).prop_map(member), (0u32..50).prop_map(|c| group("online", c))]
}

fn arb_op() -> impl Strategy<Value = MemberListOp> {
    prop_oneof![
        (0u32..300, prop::collection::vec(arb_item(), 0..40)).prop_map(|(s, items)| MemberListOp::Sync { range: [s, s + 99], items }),
        (0u32..300, arb_item()).prop_map(|(index, item)| MemberListOp::Insert { index, item }),
        (0u32..300, arb_item()).prop_map(|(index, item)| MemberListOp::Update { index, item }),
        (0u32..300).prop_map(|index| MemberListOp::Delete { index }),
        (0u32..300, 0u32..300).prop_map(|(a, b)| MemberListOp::Invalidate { range: [a, b] }),
    ]
}

fn arb_update() -> impl Strategy<Value = MemberListUpdate> {
    (prop::collection::vec(0u32..80, 0..4), prop::collection::vec(arb_op(), 0..8)).prop_map(|(counts, ops)| {
        let groups = counts.into_iter().enumerate().map(|(i, count)| MemberListGroup { id: format!("g{i}"), count }).collect();
        update(groups, ops)
    })
}

/// A consistent mutation of a synced list: the group count changes with it, as Discord sends it.
#[derive(Debug, Clone)]
enum Mutation {
    Insert(usize, u64),
    Delete(usize),
    Update(usize, u64),
    /// DELETE + INSERT in one update (a member moving within the list).
    Move(usize, usize),
}

fn arb_mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        (any::<prop::sample::Index>(), 1000u64..2000).prop_map(|(i, id)| Mutation::Insert(i.index(1 << 16), id)),
        any::<prop::sample::Index>().prop_map(|i| Mutation::Delete(i.index(1 << 16))),
        (any::<prop::sample::Index>(), 2000u64..3000).prop_map(|(i, id)| Mutation::Update(i.index(1 << 16), id)),
        (any::<prop::sample::Index>(), any::<prop::sample::Index>()).prop_map(|(a, b)| Mutation::Move(a.index(1 << 16), b.index(1 << 16))),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn member_list_never_panics(updates in prop::collection::vec(arb_update(), 1..12)) {
        let mut list = MemberList::default();
        for u in &updates {
            list.apply(u);
            let total: usize = u.groups.iter().map(|g| g.count as usize + 1).sum();
            if total > 0 {
                prop_assert!(list.rows.len() <= total, "rows {} > total {}", list.rows.len(), total);
            }
            prop_assert_eq!(list.member_count, u.member_count);
        }
    }

    #[test]
    fn sync_then_inserts_and_deletes_keep_row_counts(initial in 0usize..60, muts in prop::collection::vec(arb_mutation(), 0..40)) {
        // Reference model: header (id 0) followed by members.
        let mut model: Vec<u64> = std::iter::once(0).chain((0..initial as u64).map(|i| 100 + i)).collect();
        let mut list = MemberList::default();
        let items: Vec<MemberListItem> = model.iter().map(|&id| if id == 0 { group("online", initial as u32) } else { member(id) }).collect();
        let groups = |n: usize| vec![MemberListGroup { id: "online".into(), count: n as u32 }];
        list.apply(&update(groups(initial), vec![MemberListOp::Sync { range: [0, 99], items }]));
        prop_assert_eq!(list.rows.iter().map(row_id).collect::<Vec<_>>(), model.iter().map(|&i| Some(i)).collect::<Vec<_>>());

        for m in muts {
            let members = model.len() - 1;
            let ops = match m {
                Mutation::Insert(i, id) => {
                    let at = 1 + i % (members + 1);
                    model.insert(at, id);
                    vec![MemberListOp::Insert { index: at as u32, item: member(id) }]
                }
                Mutation::Delete(_) | Mutation::Update(..) | Mutation::Move(..) if members == 0 => continue,
                Mutation::Delete(i) => {
                    let at = 1 + i % members;
                    model.remove(at);
                    vec![MemberListOp::Delete { index: at as u32 }]
                }
                Mutation::Update(i, id) => {
                    let at = 1 + i % members;
                    model[at] = id;
                    vec![MemberListOp::Update { index: at as u32, item: member(id) }]
                }
                Mutation::Move(a, b) => {
                    let from = 1 + a % members;
                    let id = model.remove(from);
                    let to = 1 + b % members;
                    model.insert(to, id);
                    vec![MemberListOp::Delete { index: from as u32 }, MemberListOp::Insert { index: to as u32, item: member(id) }]
                }
            };
            list.apply(&update(groups(model.len() - 1), ops));
            prop_assert_eq!(list.rows.len(), model.len(), "row count follows the group counts");
            let got: Vec<Option<u64>> = list.rows.iter().map(row_id).collect();
            prop_assert_eq!(got, model.iter().map(|&i| Some(i)).collect::<Vec<_>>());
        }
    }
}

// ---------------------------------------------------------------- message cache

const CH: Snowflake = Snowflake(ids::GENERAL);

fn store() -> Store {
    let (ev, err) = Event::parse("READY", &ReadyBuilder::standard().build());
    assert!(err.is_none());
    let mut s = Store::new();
    s.apply(&ev);
    s
}

fn msg(id: u64, author: u64, nonce: Option<String>) -> Message {
    Message {
        id: Snowflake(id),
        channel_id: CH,
        author: User { id: Snowflake(author), username: format!("u{author}"), ..Default::default() },
        content: format!("m{id}"),
        nonce: nonce.map(Value::String),
        ..Default::default()
    }
}

#[derive(Debug, Clone)]
enum CacheOp {
    History {
        ids: Vec<u64>,
        latest: bool,
    },
    Create {
        id: u64,
        mine: bool,
    },
    Delete(u64),
    Send(u64),
    /// The server echo of the `n`th pending send.
    Resolve(prop::sample::Index, u64),
}

fn arb_cache_op() -> impl Strategy<Value = CacheOp> {
    prop_oneof![
        1 => (prop::collection::vec(1u64..3000, 0..60), any::<bool>()).prop_map(|(ids, latest)| CacheOp::History { ids, latest }),
        4 => (1u64..3000, any::<bool>()).prop_map(|(id, mine)| CacheOp::Create { id, mine }),
        1 => (1u64..3000).prop_map(CacheOp::Delete),
        1 => (3000u64..4000).prop_map(CacheOp::Send),
        1 => (any::<prop::sample::Index>(), 4000u64..5000).prop_map(|(i, id)| CacheOp::Resolve(i, id)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn message_cache_is_sorted_deduplicated_and_bounded(ops in prop::collection::vec(arb_cache_op(), 1..80)) {
        let mut s = store();
        let mut nonces: Vec<String> = vec![];
        for op in ops {
            let mut created = false;
            match op {
                CacheOp::History { ids, latest } => {
                    let n = ids.len();
                    s.insert_history(CH, ids.into_iter().map(|i| msg(i, ids::FRIEND, None)).collect(), 50, latest);
                    if n < 50 {
                        prop_assert!(s.messages[&CH].reached_start);
                    }
                }
                CacheOp::Create { id, mine } => {
                    let author = if mine { ids::ME } else { ids::FRIEND };
                    s.apply(&Event::MessageCreate(Box::new(msg(id, author, None))));
                    created = true;
                }
                CacheOp::Delete(id) => {
                    s.apply(&Event::MessageDelete(MessageDelete { id: Snowflake(id), channel_id: CH, guild_id: None }));
                    prop_assert!(s.message(CH, Snowflake(id)).is_none());
                }
                CacheOp::Send(n) => {
                    let nonce = n.to_string();
                    s.add_pending(CH, &nonce, "pending", None);
                    prop_assert!(s.message(CH, Snowflake(n)).is_some(), "local echo is visible");
                    nonces.push(nonce);
                }
                CacheOp::Resolve(i, id) if !nonces.is_empty() => {
                    let nonce = nonces.remove(i.index(nonces.len()));
                    let local: u64 = nonce.parse().unwrap();
                    s.apply(&Event::MessageCreate(Box::new(msg(id, ids::ME, Some(nonce.clone())))));
                    prop_assert!(!s.pending.contains_key(&nonce));
                    prop_assert!(s.message(CH, Snowflake(local)).is_none() || nonces.contains(&nonce), "local echo replaced");
                    created = true;
                }
                CacheOp::Resolve(..) => {}
            }
            let Some(cm) = s.messages.get(&CH) else { continue };
            let ids: Vec<u64> = cm.messages.iter().map(|m| m.id.0).collect();
            prop_assert!(ids.windows(2).all(|w| w[0] < w[1]), "strictly ascending, no duplicates: {:?}", ids);
            prop_assert!(cm.messages.iter().all(|m| m.channel_id == CH));
            if created && cm.has_latest {
                prop_assert!(cm.messages.len() <= MESSAGE_CACHE_LIMIT);
            }
        }
    }
}

#[test]
fn trimming_keeps_the_newest_messages() {
    let mut s = store();
    s.insert_history(CH, (1..=10).map(|i| msg(i, ids::FRIEND, None)).collect(), 50, true);
    assert!(s.messages[&CH].reached_start);
    for id in 11..=700u64 {
        s.apply(&Event::MessageCreate(Box::new(msg(id, ids::FRIEND, None))));
    }
    let cm = &s.messages[&CH];
    assert_eq!(cm.messages.len(), MESSAGE_CACHE_LIMIT);
    assert_eq!(cm.messages.first().unwrap().id.0, 101);
    assert_eq!(cm.messages.last().unwrap().id.0, 700);
    assert!(!cm.reached_start, "older history must be fetched again after trimming");
}

#[test]
fn history_and_live_copies_are_deduplicated() {
    let mut s = store();
    s.insert_history(CH, vec![msg(5, ids::FRIEND, None), msg(3, ids::FRIEND, None)], 50, true);
    s.apply(&Event::MessageCreate(Box::new(msg(5, ids::FRIEND, None))));
    s.insert_history(CH, vec![msg(5, ids::FRIEND, None), msg(4, ids::FRIEND, None)], 50, false);
    let ids: Vec<u64> = s.messages[&CH].messages.iter().map(|m| m.id.0).collect();
    assert_eq!(ids, vec![3, 4, 5]);
}

#[test]
fn live_messages_are_not_cached_without_the_latest_page() {
    let mut s = store();
    let id = ids::GENERAL + 5_000;
    s.apply(&Event::MessageCreate(Box::new(msg(id, ids::FRIEND, None))));
    assert!(s.messages.get(&CH).is_none_or(|cm| cm.messages.is_empty()), "no gap-filling into an unloaded channel");
    assert_eq!(s.channels[&CH].last_message_id, Some(Snowflake(id)));
    assert!(s.is_unread(CH));
}
