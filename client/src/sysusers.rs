//! Enumerate real OS login users so the agent can report them at enrollment /
//! heartbeat and map per-user policy (zero-trust: policy is per person).

use serde::{Deserialize, Serialize};

/// Conventional range for interactive login accounts on most distros.
const UID_MIN: u32 = 1000;
const UID_MAX: u32 = 60000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsUser {
    pub username: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
}

/// All human login users (UID in [1000, 60000)), sorted by uid.
pub fn login_users() -> Vec<OsUser> {
    let mut out = Vec::new();
    // Safety: `all_users` is unsafe because it isn't reentrant; we call it once,
    // synchronously, from a single place.
    let iter = unsafe { users::all_users() };
    for u in iter {
        let uid = u.uid();
        if (UID_MIN..UID_MAX).contains(&uid) {
            let username = u.name().to_string_lossy().to_string();
            // The `users` crate doesn't expose GECOS; use the username as the
            // display name (the admin can rename per device_user server-side).
            let display_name = username.clone();
            out.push(OsUser {
                username,
                display_name,
                uid: Some(uid),
            });
        }
    }
    out.sort_by_key(|u| u.uid.unwrap_or(0));
    out.dedup_by(|a, b| a.username == b.username);
    out
}

/// Resolve a username to its UID (needed to locate its cgroup slice).
pub fn uid_of(username: &str) -> Option<u32> {
    users::get_user_by_name(username).map(|u| u.uid())
}
