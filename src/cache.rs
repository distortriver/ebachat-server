//! Mattermost's in-memory caches in front of the database: the store's `localcachelayer` and the platform's
//! session and status caches, with Go's sizes and lifetimes. Writes invalidate them; there is one node, so no
//! cluster messages.

// Based on Mattermost server/channels/store/localcachelayer/layer.go and channels/app/platform/service.go,
// Copyright (c) 2015-present Mattermost, Inc. Licensed under AGPL-3.0.

use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::model::{
    Channel, Emoji, FileInfo, Reaction, Role, Scheme, Session, Status, StringMap, User,
};

/// An LRU of `size` entries that also drop `ttl` after they were put, as Go's `cache.LRU`.
pub struct Lru<K: Hash + Eq, V> {
    m: Mutex<lru::LruCache<K, (V, Instant)>>,
    ttl: Option<Duration>,
}

impl<K: Hash + Eq, V: Clone> Lru<K, V> {
    pub fn new(size: usize, ttl: Option<Duration>) -> Self {
        Lru { m: Mutex::new(lru::LruCache::new(NonZeroUsize::new(size).unwrap())), ttl }
    }
    pub fn get<Q>(&self, k: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let mut m = self.m.lock().unwrap();
        let (v, at) = m.get(k)?;
        if self.ttl.is_some_and(|t| at.elapsed() > t) {
            m.pop(k);
            return None;
        }
        Some(v.clone())
    }
    pub fn put(&self, k: K, v: V) {
        self.m.lock().unwrap().put(k, (v, Instant::now()));
    }
    /// An entry that lives `ttl` (shorter than the cache's own), as Go's `SetWithExpiry`.
    pub fn put_for(&self, k: K, v: V, ttl: Duration) {
        let at = match self.ttl {
            Some(t) if t > ttl => Instant::now().checked_sub(t - ttl).unwrap_or_else(Instant::now),
            _ => Instant::now(),
        };
        self.m.lock().unwrap().put(k, (v, at));
    }
    pub fn remove<Q>(&self, k: &Q)
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.m.lock().unwrap().pop(k);
    }
    pub fn clear(&self) {
        self.m.lock().unwrap().clear();
    }
    /// Drops every entry `f` says to.
    pub fn remove_where(&self, f: impl Fn(&K, &V) -> bool)
    where
        K: Clone,
    {
        let mut m = self.m.lock().unwrap();
        let gone: Vec<K> = m.iter().filter(|(k, (v, _))| f(k, v)).map(|(k, _)| k.clone()).collect();
        for k in gone {
            m.pop(&k);
        }
    }
}

const MIN: Duration = Duration::from_secs(60);
const SESSION_CACHE_SIZE: usize = 35000;
const CHANNEL_CACHE_SIZE: usize = 25000;

/// A user's channel memberships: channel id → roles (`GetAllChannelMembersForUser`).
pub type MemberRoles = Arc<HashMap<String, String>>;

pub struct Caches {
    // platform
    pub session: Lru<String, Arc<Session>>,
    pub status: Lru<String, Status>,
    // app
    pub seen_pending_post_ids: Lru<String, String>,
    pub open_graph: Lru<String, Arc<serde_json::Value>>,
    // store: users
    pub user: Lru<String, Arc<User>>,
    pub all_profiles: Lru<(), Arc<Vec<Arc<User>>>>,
    pub profiles_in_channel: Lru<String, Arc<HashMap<String, Arc<User>>>>,
    // store: channels
    pub channel: Lru<String, Arc<Channel>>,
    pub channel_by_name: Lru<String, Arc<Channel>>,
    pub member_count: Lru<String, i64>,
    pub guest_count: Lru<String, i64>,
    pub pinned_count: Lru<String, i64>,
    pub members_for_user: Lru<String, MemberRoles>,
    pub members_notify_props: Lru<String, Arc<HashMap<String, StringMap>>>,
    // store: posts
    pub last_post_time: Lru<String, i64>,
    /// First pages (`{channel}{30|60}`) as `GetPosts` returned them.
    pub last_posts: Lru<String, Arc<serde_json::Value>>,
    // store: teams
    pub team_ids_for_user: Lru<String, Arc<Vec<String>>>,
    // store: roles, schemes
    pub role: Lru<String, Arc<Role>>,
    pub scheme: Lru<String, Arc<Scheme>>,
    // store: post extras
    pub reactions: Lru<String, Arc<Vec<Reaction>>>,
    pub file_infos: Lru<String, Arc<Vec<FileInfo>>>,
    pub emoji_by_id: Lru<String, Option<Arc<Emoji>>>,
    pub emoji_by_name: Lru<String, Option<Arc<Emoji>>>,
    pub terms_of_service: Lru<String, Arc<serde_json::Value>>,
}

impl Caches {
    pub fn new(session_minutes: u64) -> Caches {
        let m = |n: u64| Some(MIN * n as u32);
        Caches {
            session: Lru::new(SESSION_CACHE_SIZE, m(session_minutes.max(1))),
            status: Lru::new(SESSION_CACHE_SIZE, m(30)),
            seen_pending_post_ids: Lru::new(25000, Some(Duration::from_secs(30))),
            open_graph: Lru::new(10000, m(60)),
            user: Lru::new(20000, m(30)),
            all_profiles: Lru::new(1, m(30)),
            profiles_in_channel: Lru::new(CHANNEL_CACHE_SIZE, m(15)),
            channel: Lru::new(CHANNEL_CACHE_SIZE, m(15)),
            channel_by_name: Lru::new(CHANNEL_CACHE_SIZE, m(15)),
            member_count: Lru::new(CHANNEL_CACHE_SIZE, m(30)),
            guest_count: Lru::new(CHANNEL_CACHE_SIZE, m(30)),
            pinned_count: Lru::new(CHANNEL_CACHE_SIZE, m(30)),
            members_for_user: Lru::new(SESSION_CACHE_SIZE, m(15)),
            members_notify_props: Lru::new(SESSION_CACHE_SIZE, m(30)),
            last_post_time: Lru::new(25000, m(15)),
            last_posts: Lru::new(20000, m(30)),
            team_ids_for_user: Lru::new(20000, m(30)),
            role: Lru::new(20000, m(30)),
            scheme: Lru::new(20000, m(30)),
            reactions: Lru::new(20000, m(30)),
            file_infos: Lru::new(25000, m(30)),
            emoji_by_id: Lru::new(5000, m(30)),
            emoji_by_name: Lru::new(5000, m(30)),
            terms_of_service: Lru::new(20000, m(30)),
        }
    }

    // ---- Go's invalidation entry points ----

    /// `InvalidateProfileCacheForUser` and `InvalidateProfilesInChannelCacheByUser`.
    pub fn invalidate_user(&self, user_id: &str) {
        self.user.remove(user_id);
        self.all_profiles.clear();
        self.profiles_in_channel.remove_where(|_, m| m.contains_key(user_id));
    }
    /// `InvalidateAllChannelMembersForUser`.
    pub fn invalidate_members_for_user(&self, user_id: &str) {
        self.members_for_user.remove(user_id);
        self.members_for_user.remove(&format!("{user_id}_deleted"));
    }
    /// `InvalidateChannel` and `InvalidateChannelByName`.
    pub fn invalidate_channel(&self, c: &Channel) {
        self.channel.remove(&c.id);
        self.channel_by_name.remove(&format!("{}{}", c.team_id, c.name));
    }
    /// After a membership change in a channel: counts, notify props, profiles in it.
    pub fn invalidate_channel_members(&self, channel_id: &str) {
        self.member_count.remove(channel_id);
        self.guest_count.remove(channel_id);
        self.members_notify_props.remove(channel_id);
        self.profiles_in_channel.remove(channel_id);
    }
    /// `InvalidateLastPostTimeCache`: the time and the cached first pages.
    pub fn invalidate_last_post_time(&self, channel_id: &str) {
        self.last_post_time.remove(channel_id);
        self.last_posts.remove(&format!("{channel_id}30"));
        self.last_posts.remove(&format!("{channel_id}60"));
    }
    pub fn invalidate_post_extras(&self, post_id: &str) {
        self.reactions.remove(post_id);
        self.file_infos.remove(post_id);
    }
}

/// Sorted for stable output.
pub fn sorted<V: Clone>(m: &HashMap<String, V>) -> BTreeMap<String, V> {
    m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}
